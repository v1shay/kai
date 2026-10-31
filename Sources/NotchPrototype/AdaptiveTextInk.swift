import AppKit

/// Foreground text colors only. No background, layer opacity, border, shape,
/// image, font, frame, appearance or glass property is ever written here.
@MainActor final class AdaptiveTextInk {
    weak var host:NSView?
    private var target:UInt32?,epoch=0,started=Date(),duration=0.0,applying=false
    private var task:DispatchWorkItem?
    private final class Source:NSObject{var original:NSColor?,applied:NSColor?,from:NSColor?,epoch = -1}
    private let fields=NSMapTable<NSView,Source>(keyOptions:.weakMemory,valueOptions:.strongMemory)
    private let carets=NSMapTable<NSTextView,Source>(keyOptions:.weakMemory,valueOptions:.strongMemory)
    private static let original=NSAttributedString.Key("kaiTextOriginal"),applied=NSAttributedString.Key("kaiTextApplied"),from=NSAttributedString.Key("kaiTextFrom"),epochKey=NSAttributedString.Key("kaiTextEpoch")
    init(host:NSView){self.host=host}
    func apply(color:UInt32,animated:Bool){
        if target != color{target=color;epoch+=1;started=Date();duration=animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.24:0;task?.cancel();task=nil}
        refresh();schedule()
    }
    private func schedule(){
        guard target != nil,duration>0,Date().timeIntervalSince(started)<duration,task==nil else{return}
        let generation=epoch;let work=DispatchWorkItem{[weak self] in guard let self,self.epoch==generation,self.target != nil else{return};self.task=nil;self.refresh();self.schedule()};task=work;DispatchQueue.main.asyncAfter(deadline:.now()+1.0/60,execute:work)
    }
    private func adjusted(_ source:NSColor)->NSColor{
        guard let color=source.usingColorSpace(.sRGB),let target else{return source}
        let original=[color.redComponent,color.greenComponent,color.blueComponent]
        let ink=[CGFloat((target>>16)&255)/255,CGFloat((target>>8)&255)/255,CGFloat(target&255)/255]
        let neutral=(original.max() ?? 0)-(original.min() ?? 0)<0.10
        let rgb=(0..<3).map{neutral ? ink[$0]:original[$0]*0.4+ink[$0]*0.6}
        return NSColor(srgbRed:rgb[0],green:rgb[1],blue:rgb[2],alpha:color.alphaComponent)
    }
    private func mix(_ from:NSColor,_ to:NSColor,_ t:Double)->NSColor{
        guard let a=from.usingColorSpace(.sRGB),let b=to.usingColorSpace(.sRGB) else{return to}
        func lerp(_ a:CGFloat,_ b:CGFloat)->CGFloat{a+(b-a)*t}
        return NSColor(srgbRed:lerp(a.redComponent,b.redComponent),green:lerp(a.greenComponent,b.greenComponent),blue:lerp(a.blueComponent,b.blueComponent),alpha:lerp(a.alphaComponent,b.alphaComponent))
    }
    func refresh(){
        guard !applying,target != nil,let host else{return};applying=true;defer{applying=false}
        let x=duration>0 ? min(1,max(0,Date().timeIntervalSince(started)/duration)):1,t=x*x*(3-2*x)
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        walk(host,t:t,restore:false);if let layer=host.layer{walk(layer,t:t,restore:false)}
    }
    func restore(){
        guard target != nil,!applying,let host else{return};task?.cancel();task=nil;applying=true;defer{applying=false};epoch+=1
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        walk(host,t:1,restore:true);if let layer=host.layer{walk(layer,t:1,restore:true)};target=nil;fields.removeAllObjects();carets.removeAllObjects()
    }
    private func next(_ current:NSColor,source:Source,t:Double,restore:Bool)->NSColor{
        if source.applied==nil || current != source.applied{source.original=current;source.from=current}
        guard let original=source.original else{return current}
        if restore{return original}
        if source.epoch != epoch{source.from=current;source.epoch=epoch}
        let value=mix(source.from ?? current,adjusted(original),t);source.applied=value;return value
    }
    private func walk(_ view:NSView,t:Double,restore:Bool){
        if let field=view as? NSTextField,let color=field.textColor{let source=fields.object(forKey:field) ?? Source();field.textColor=next(color,source:source,t:t,restore:restore);if !restore{fields.setObject(source,forKey:field)}}
        if let text=view as? NSTextView{
            let source=carets.object(forKey:text) ?? Source();text.insertionPointColor=next(text.insertionPointColor,source:source,t:t,restore:restore);if !restore{carets.setObject(source,forKey:text)}
            if let storage=text.textStorage,storage.length>0{
                var changes=[(NSRange,NSColor,NSColor,NSColor)]()
                storage.enumerateAttributes(in:NSRange(location:0,length:storage.length)){attributes,range,_ in
                    guard let current=attributes[.foregroundColor] as? NSColor else{return}
                    let previous=attributes[Self.applied] as? NSColor
                    let original=(previous != nil && previous != current) ? current:(attributes[Self.original] as? NSColor ?? current)
                    let from=(attributes[Self.epochKey] as? Int)==epoch ? (attributes[Self.from] as? NSColor ?? current):current
                    let result=restore ? original:mix(from,adjusted(original),t)
                    if result != current || (!restore && attributes[Self.original]==nil) || (restore && attributes[Self.original] != nil){changes.append((range,original,from,result))}
                }
                if !changes.isEmpty{storage.beginEditing();for(range,original,from,result)in changes{storage.addAttribute(.foregroundColor,value:result,range:range);if restore{for key in [Self.original,Self.applied,Self.from,Self.epochKey]{storage.removeAttribute(key,range:range)}}else{storage.addAttributes([Self.original:original,Self.applied:result,Self.from:from,Self.epochKey:epoch],range:range)}};storage.endEditing()}
            }
        }
        for child in view.subviews{walk(child,t:t,restore:restore)}
    }
    private func walk(_ layer:CALayer,t:Double,restore:Bool){
        if let text=layer as? CATextLayer,let current=text.foregroundColor,let color=NSColor(cgColor:current){
            let previous=AdaptiveColorMath.cgColor(layer.value(forKey:"kaiTextApplied")),stored=AdaptiveColorMath.cgColor(layer.value(forKey:"kaiTextOriginal"))
            let original=(previous != nil && previous != current) ? current:(stored ?? current)
            let from=(layer.value(forKey:"kaiTextEpoch") as? Int)==epoch ? (AdaptiveColorMath.cgColor(layer.value(forKey:"kaiTextFrom")) ?? current):current
            let result=restore ? original:mix(NSColor(cgColor:from) ?? color,adjusted(NSColor(cgColor:original) ?? color),t).cgColor
            text.foregroundColor=result
            if restore{for key in ["kaiTextOriginal","kaiTextApplied","kaiTextFrom","kaiTextEpoch"]{layer.setValue(nil,forKey:key)}}else{layer.setValue(original,forKey:"kaiTextOriginal");layer.setValue(result,forKey:"kaiTextApplied");layer.setValue(from,forKey:"kaiTextFrom");layer.setValue(epoch,forKey:"kaiTextEpoch")}
        }
        // Masks are not part of sublayers and are intentionally never traversed.
        for child in layer.sublayers ?? []{walk(child,t:t,restore:restore)}
    }
}
