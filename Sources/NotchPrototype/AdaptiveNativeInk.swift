import AppKit

/// Restorable semantic ink only. Image artwork, sprite atlases, masks, glyph
/// geometry, selection, text contents and pet animation timelines stay intact.
@MainActor final class AdaptiveNativeInk {
    weak var host:NSView?
    private var applying=false,lightInk=true,range=(0.0,0.0),target=4.5
    private var colours=[String:NSColor]()
    private let views=NSMapTable<NSView,Source>(keyOptions:.weakMemory,valueOptions:.strongMemory)
    private let carets=NSMapTable<NSTextView,Source>(keyOptions:.weakMemory,valueOptions:.strongMemory)
    private var originalAppearance:NSAppearance?,appliedAppearance:NSAppearance?,appearanceCaptured=false
    private final class Source:NSObject {var original:NSColor?,applied:NSColor?}
    private static let original=NSAttributedString.Key("kaiAdaptiveOriginalInk")
    private static let applied=NSAttributedString.Key("kaiAdaptiveAppliedInk")
    init(host:NSView){self.host=host}
    func apply(ink:String,backgrounds:[[Double]],target:Double){
        guard !applying,let host else{return};applying=true;defer{applying=false};lightInk=ink != "dark"
        let values=backgrounds.map(AdaptiveColorMath.luminance)
        let next=(values.min() ?? 0,values.max() ?? 0)
        if abs(next.0-range.0)>0.008 || abs(next.1-range.1)>0.008 || abs(self.target-target)>0.01{colours.removeAll()}
        range=next;self.target=target
        if !appearanceCaptured{originalAppearance=host.appearance;appearanceCaptured=true}
        let appearance=NSAppearance(named:lightInk ? .darkAqua:.aqua)
        if host.appearance != appearance{host.appearance=appearance};appliedAppearance=appearance
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        walk(host,restore:false);if let layer=host.layer{walk(layer,restore:false)}
    }
    func restore(){
        guard !applying,appearanceCaptured,let host else{return};applying=true;defer{applying=false}
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        walk(host,restore:true);if let layer=host.layer{walk(layer,restore:true)};views.removeAllObjects();carets.removeAllObjects()
        if appearanceCaptured,host.appearance == appliedAppearance{host.appearance=originalAppearance}
        appearanceCaptured=false;originalAppearance=nil;appliedAppearance=nil
    }
    private func adjusted(_ value:NSColor)->NSColor{
        guard let rgb=value.usingColorSpace(.sRGB) else{return value}
        let alpha=rgb.alphaComponent>=0.25 ? 1:rgb.alphaComponent
        let key="\(lightInk)|\(rgb.redComponent)|\(rgb.greenComponent)|\(rgb.blueComponent)|\(alpha)"
        if let cached=colours[key]{return cached}
        let original=[Double(rgb.redComponent),Double(rgb.greenComponent),Double(rgb.blueComponent)]
        let endpoint=lightInk ? [249.0/255,250.0/255,251.0/255]:[16.0/255,19.0/255,24.0/255]
        func blended(_ t:Double)->[Double]{(0..<3).map{original[$0]*(1-t)+endpoint[$0]*t}}
        func good(_ c:[Double])->Bool{let l=AdaptiveColorMath.luminance(c);return !(l>=range.0 && l<=range.1) && min(AdaptiveColorMath.contrast(l,range.0),AdaptiveColorMath.contrast(l,range.1))>=target}
        let neutral=max(original[0],original[1],original[2])-min(original[0],original[1],original[2])<0.035
        var result=original
        if neutral{result=endpoint}else if !good(original){var low=0.0,high=1.0;for _ in 0..<18{let mid=(low+high)/2;if good(blended(mid)){high=mid}else{low=mid}};result=blended(high)}
        let color=NSColor(srgbRed:result[0],green:result[1],blue:result[2],alpha:alpha)
        if colours.count>=256{colours.removeAll()};colours[key]=color;return color
    }
    private func color(_ layer:CALayer,key:String,current:CGColor?,restore:Bool,set:(CGColor?)->Void){
        let sourceKey="kaiAdaptiveSource_"+key,appliedKey="kaiAdaptiveApplied_"+key
        let previous=AdaptiveColorMath.cgColor(layer.value(forKey:appliedKey))
        var source=AdaptiveColorMath.cgColor(layer.value(forKey:sourceKey))
        if !restore,let current,(previous == nil || current != previous){source=current;layer.setValue(current,forKey:sourceKey)}
        if restore{if let source,current==previous{set(source)};layer.setValue(nil,forKey:sourceKey);layer.setValue(nil,forKey:appliedKey);return}
        guard let source,let original=NSColor(cgColor:source) else{return}
        let value=adjusted(original).cgColor;if current != value{set(value)};layer.setValue(value,forKey:appliedKey)
    }
    private func walk(_ layer:CALayer,restore:Bool){
        guard layer.name != "kai.adaptive-guard" else{return}
        if let text=layer as? CATextLayer{color(layer,key:"foreground",current:text.foregroundColor,restore:restore){text.foregroundColor=$0}}
        if let shape=layer as? CAShapeLayer{color(layer,key:"stroke",current:shape.strokeColor,restore:restore){shape.strokeColor=$0}}
        // Only semitransparent neutral control fills. Opaque artwork and the
        // physical compact-notch black surface never change appearance.
        if let background=layer.backgroundColor,let rgb=NSColor(cgColor:background)?.usingColorSpace(.sRGB),rgb.alphaComponent<0.30,max(rgb.redComponent,rgb.greenComponent,rgb.blueComponent)-min(rgb.redComponent,rgb.greenComponent,rgb.blueComponent)<0.035{
            let sourceKey="kaiAdaptiveSource_fill",appliedKey="kaiAdaptiveApplied_fill"
            let previous=AdaptiveColorMath.cgColor(layer.value(forKey:appliedKey))
            var source=AdaptiveColorMath.cgColor(layer.value(forKey:sourceKey))
            if !restore,(previous == nil || background != previous){source=background;layer.setValue(background,forKey:sourceKey)}
            if restore{if let source,background==previous{layer.backgroundColor=source};layer.setValue(nil,forKey:sourceKey);layer.setValue(nil,forKey:appliedKey)}
            else{let value=(lightInk ? NSColor.white:NSColor.black).withAlphaComponent(rgb.alphaComponent).cgColor;layer.backgroundColor=value;layer.setValue(value,forKey:appliedKey)}
        }
        template(layer,restore:restore)
        for child in layer.sublayers ?? []{walk(child,restore:restore)}
    }
    private func template(_ layer:CALayer,restore:Bool){
        let sourceKey="kaiAdaptiveSource_image",appliedKey="kaiAdaptiveApplied_image",inkKey="kaiAdaptiveImageInk"
        let applied=AdaptiveColorMath.cgImage(layer.value(forKey:appliedKey)),current=AdaptiveColorMath.cgImage(layer.contents)
        if restore{if let original=layer.value(forKey:sourceKey),let applied,applied === current{layer.contents=original};layer.setValue(nil,forKey:sourceKey);layer.setValue(nil,forKey:appliedKey);layer.setValue(nil,forKey:inkKey);return}
        if let applied,applied !== current{layer.setValue(nil,forKey:sourceKey);layer.setValue(nil,forKey:appliedKey);layer.setValue(nil,forKey:inkKey)}
        guard layer.contentsRect == CGRect(x:0,y:0,width:1,height:1),layer.animation(forKey:"inlinePet")==nil,!(layer.name ?? "").contains("pet"),let image=AdaptiveColorMath.cgImage(layer.contents),image.width<=48,image.height<=48 else{return}
        let source:CGImage
        if let applied=AdaptiveColorMath.cgImage(layer.value(forKey:appliedKey)),applied === image,let original=AdaptiveColorMath.cgImage(layer.value(forKey:sourceKey)){if layer.value(forKey:inkKey) as? Bool == lightInk{return};source=original}else{source=image}
        let width=source.width,height=source.height
        var bytes=[UInt8](repeating:0,count:width*height*4)
        let valid=bytes.withUnsafeMutableBytes{raw -> Bool in
            guard let context=CGContext(data:raw.baseAddress,width:width,height:height,bitsPerComponent:8,bytesPerRow:width*4,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue|CGBitmapInfo.byteOrder32Big.rawValue) else{return false}
            context.draw(source,in:CGRect(x:0,y:0,width:width,height:height));return true
        }
        guard valid else{return};var visible=false
        for at in stride(from:0,to:bytes.count,by:4) where bytes[at+3]>24{
            visible=true;let r=Int(bytes[at]),g=Int(bytes[at+1]),b=Int(bytes[at+2]),a=Int(bytes[at+3]);if max(r,g,b)-min(r,g,b)>4 || r*100<a*80{return}
        }
        guard visible else{return};let ink=lightInk ? [UInt8(249),250,251]:[UInt8(16),19,24]
        for at in stride(from:0,to:bytes.count,by:4){for c in 0..<3{bytes[at+c]=UInt8(Int(ink[c])*Int(bytes[at+3])/255)}}
        guard let data=CFDataCreate(nil,bytes,bytes.count),let provider=CGDataProvider(data:data),let result=CGImage(width:width,height:height,bitsPerComponent:8,bitsPerPixel:32,bytesPerRow:width*4,space:CGColorSpace(name:CGColorSpace.sRGB)!,bitmapInfo:CGBitmapInfo(rawValue:CGImageAlphaInfo.premultipliedLast.rawValue|CGBitmapInfo.byteOrder32Big.rawValue),provider:provider,decode:nil,shouldInterpolate:true,intent:.defaultIntent) else{return}
        layer.setValue(source,forKey:sourceKey);layer.contents=result;layer.setValue(result,forKey:appliedKey);layer.setValue(lightInk,forKey:inkKey)
    }
    private func walk(_ view:NSView,restore:Bool){
        if let field=view as? NSTextField{let source=views.object(forKey:field) ?? Source();if !restore,let value=field.textColor,(source.applied == nil || value != source.applied){source.original=value};if let original=source.original,(!restore || field.textColor == source.applied){let next=restore ? original:adjusted(original);field.textColor=next;source.applied=next};if !restore{views.setObject(source,forKey:field)}}
        if let button=view as? NSButton,let value=button.contentTintColor{let source=views.object(forKey:button) ?? Source();if !restore,(source.applied == nil || value != source.applied){source.original=value};if let original=source.original,(!restore || value == source.applied){let next=restore ? original:adjusted(original);button.contentTintColor=next;source.applied=next};if !restore{views.setObject(source,forKey:button)}}
        if let text=view as? NSTextView{
            let source=carets.object(forKey:text) ?? Source(),current=text.insertionPointColor
            if !restore,(source.applied == nil || current != source.applied){source.original=current}
            if let original=source.original,(!restore || current == source.applied){let next=restore ? original:adjusted(original);if current != next{text.insertionPointColor=next};source.applied=next}
            if !restore{carets.setObject(source,forKey:text)}
        }
        if let text=view as? NSTextView,let storage=text.textStorage,storage.length>0{
            var changes=[(NSRange,NSColor)]()
            storage.enumerateAttributes(in:NSRange(location:0,length:storage.length)){attributes,range,_ in
                guard let current=attributes[.foregroundColor] as? NSColor else{return}
                let applied=attributes[Self.applied] as? NSColor
                let original=(applied != nil && applied != current) ? current:(attributes[Self.original] as? NSColor ?? current)
                let next=restore ? original:adjusted(original)
                if current != next || (!restore && attributes[Self.original]==nil) || (restore && attributes[Self.original] != nil){changes.append((range,original))}
            }
            if !changes.isEmpty{storage.beginEditing();for(range,original)in changes{if restore{storage.removeAttribute(Self.original,range:range);storage.removeAttribute(Self.applied,range:range);storage.addAttribute(.foregroundColor,value:original,range:range)}else{storage.addAttributes([Self.original:original,Self.applied:adjusted(original),.foregroundColor:adjusted(original)],range:range)}};storage.endEditing()}
        }
        for child in view.subviews{walk(child,restore:restore)}
    }
}
