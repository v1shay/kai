import AppKit

/// Same four vectors are bundled with Code; these names are model families,
/// never guesses from a provider's arbitrary model label.
enum ModelIdentity {
    static func glyph(_ model:String)->String? {
        let tokens=Set(model.lowercased().split{!$0.isLetter && !$0.isNumber}.map(String.init))
        guard tokens.contains("gpt") || tokens.contains("openai") else{return nil}
        for (family,glyph) in [("luna","model-moon"),("terra","model-earth"),("sol","model-sun"),("astra","model-star")] where tokens.contains(family){return glyph}
        return nil
    }
    static func title(_ model:String)->String {
        guard glyph(model) != nil,model.lowercased().hasPrefix("gpt-") else{return model}
        let parts=model.split(separator:"-",maxSplits:2).map(String.init)
        guard parts.count==3 else{return model}
        return "GPT-"+parts[1]+" "+parts[2].replacingOccurrences(of:"-",with:" ").capitalized
    }
    @MainActor private static let images=NSCache<NSString,NSImage>()
    @MainActor static func image(_ model:String)->NSImage? {
        guard let name=glyph(model) else{return nil}
        if let image=images.object(forKey:name as NSString){return image}
        let bundled=Bundle.main.resourceURL?.appendingPathComponent("providers/\(name).svg")
        let local=URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("integrations/Code/assets/\(name).svg")
        guard let source=(bundled.flatMap{NSImage(contentsOf:$0)} ?? NSImage(contentsOf:local)) else{return nil}
        let image=NSImage(size:CGSize(width:24,height:24),flipped:false){rect in source.draw(in:rect);NSColor.white.setFill();rect.fill(using:.sourceIn);return true}
        image.isTemplate=true;images.countLimit=4;images.setObject(image,forKey:name as NSString);return image
    }
}

enum EffortAppearance {
    static let stops:[UInt32]=[0xb6dfff,0x9692ff,0xe879ce,0xff785c,0xff5565]
    static func color(_ fraction:CGFloat)->NSColor {
        let scaled=min(1,max(0,fraction))*CGFloat(stops.count-1),index=min(stops.count-2,Int(scaled)),t=scaled-CGFloat(index)
        func channel(_ value:UInt32,_ shift:UInt32)->CGFloat{CGFloat((value>>shift)&255)/255}
        let a=stops[index],b=stops[index+1]
        return NSColor(srgbRed:channel(a,16)+(channel(b,16)-channel(a,16))*t,green:channel(a,8)+(channel(b,8)-channel(a,8))*t,blue:channel(a,0)+(channel(b,0)-channel(a,0))*t,alpha:1)
    }
    static func title(_ value:String)->String{switch value{case "xhigh":return "Extra high";default:return value.replacingOccurrences(of:"_",with:" ").capitalized}}
}

/// Native tracking and accessibility, with discrete provider-advertised stops.
/// Core Animation drives only a brief thumb/gradient transition; no idle timer.
@MainActor final class ReasoningSliderControl:NSControl {
    let levels:[String]
    private(set) var selected:Int?
    var onCommit:((String)->Void)?
    private let track=CALayer(),fill=CAGradientLayer(),thumb=CALayer(),heading=CATextLayer(),value=CATextLayer(),low=CATextLayer(),high=CATextLayer()
    private var ticks=[CALayer]()
    override var acceptsFirstResponder:Bool{true}
    init(levels:[String],current:String?){
        let bounded=Array(levels.prefix(16));self.levels=bounded;selected=bounded.firstIndex(of:current ?? "")
        super.init(frame:CGRect(x:0,y:0,width:252,height:98));wantsLayer=true
        setAccessibilityElement(true);setAccessibilityRole(.slider);setAccessibilityLabel("Thinking effort")
        for item in [heading,value,low,high]{item.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;item.font=NSFont.systemFont(ofSize:item===heading ? 11:10,weight:item===value ? .medium:.regular);item.fontSize=item===heading ? 11:10;item.foregroundColor=NSColor.white.withAlphaComponent(0.65).cgColor;layer?.addSublayer(item)}
        heading.string="Thinking";value.alignmentMode = .right
        track.backgroundColor=NSColor.white.withAlphaComponent(0.12).cgColor;track.cornerRadius=3;layer?.addSublayer(track)
        fill.anchorPoint=CGPoint(x:0,y:0.5);fill.startPoint=CGPoint(x:0,y:0.5);fill.endPoint=CGPoint(x:1,y:0.5);fill.cornerRadius=3;layer?.addSublayer(fill)
        for _ in self.levels{let tick=CALayer();tick.cornerRadius=1.5;layer?.addSublayer(tick);ticks.append(tick)}
        thumb.backgroundColor=NSColor.white.cgColor;thumb.cornerRadius=8;thumb.borderWidth=1;thumb.shadowOffset = .zero;thumb.shadowRadius=7;thumb.shadowOpacity=0.45;layer?.addSublayer(thumb)
        low.string=self.levels.first.map(EffortAppearance.title);high.string=self.levels.last.map(EffortAppearance.title);high.alignmentMode = .right
        render(animated:false)
    }
    required init?(coder:NSCoder){fatalError("init(coder:) has not been implemented")}
    private var rail:CGRect{CGRect(x:18,y:43,width:bounds.width-36,height:6)}
    private var fraction:CGFloat{CGFloat(selected ?? 0)/CGFloat(max(1,levels.count-1))}
    override func layout(){super.layout();render(animated:false)}
    private func render(animated:Bool){
        let rail=self.rail,color=EffortAppearance.color(fraction)
        let oldPosition=thumb.presentation()?.position ?? thumb.position,oldWidth=fill.presentation()?.bounds.width ?? fill.bounds.width,oldColors=fill.presentation()?.colors ?? fill.colors
        CATransaction.begin();CATransaction.setDisableActions(true)
        heading.frame=CGRect(x:18,y:72,width:90,height:16);value.frame=CGRect(x:98,y:72,width:bounds.width-116,height:16);value.string=selected.map{EffortAppearance.title(levels[$0])} ?? "Choose a level";value.foregroundColor=color.cgColor
        track.frame=rail;fill.frame=CGRect(x:rail.minX,y:rail.minY,width:max(6,rail.width*fraction),height:rail.height)
        fill.colors=[EffortAppearance.color(max(0,fraction-0.4)).cgColor,color.cgColor]
        thumb.frame=CGRect(x:rail.minX+rail.width*fraction-8,y:rail.midY-8,width:16,height:16);thumb.borderColor=color.withAlphaComponent(0.7).cgColor;thumb.shadowColor=color.cgColor;thumb.opacity=selected == nil ? 0.45:1
        for (i,tick) in ticks.enumerated(){tick.frame=CGRect(x:rail.minX+rail.width*CGFloat(i)/CGFloat(max(1,levels.count-1))-1.5,y:rail.midY-1.5,width:3,height:3);tick.backgroundColor=NSColor.white.withAlphaComponent(i <= (selected ?? -1) ? 0.9:0.3).cgColor}
        low.frame=CGRect(x:18,y:17,width:100,height:16);high.frame=CGRect(x:bounds.width-118,y:17,width:100,height:16)
        setAccessibilityValue(selected.map{EffortAppearance.title(levels[$0])} ?? "Unselected")
        CATransaction.commit()
        if animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            let move=CASpringAnimation(keyPath:"position");move.fromValue=oldPosition;move.toValue=thumb.position;move.mass=1;move.stiffness=290;move.damping=28;move.duration=0.34;thumb.add(move,forKey:"effortSlide")
            let width=CABasicAnimation(keyPath:"bounds.size.width");width.fromValue=oldWidth;width.toValue=fill.bounds.width;width.duration=0.28;width.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);fill.add(width,forKey:"effortFill")
            let colors=CABasicAnimation(keyPath:"colors");colors.fromValue=oldColors;colors.toValue=fill.colors;colors.duration=0.28;fill.add(colors,forKey:"effortIntensity")
        }
    }
    func select(_ index:Int,commit:Bool){guard !levels.isEmpty else{return};let next=max(0,min(levels.count-1,index));if selected != next{selected=next;render(animated:true)};if commit{onCommit?(levels[next])}}
    private func index(at point:CGPoint)->Int{Int(((point.x-rail.minX)/rail.width*CGFloat(max(1,levels.count-1))).rounded())}
    override func mouseDown(with event:NSEvent){
        window?.makeFirstResponder(self);select(index(at:convert(event.locationInWindow,from:nil)),commit:false)
        while let next=window?.nextEvent(matching:[.leftMouseDragged,.leftMouseUp]){select(index(at:convert(next.locationInWindow,from:nil)),commit:next.type == .leftMouseUp);if next.type == .leftMouseUp{break}}
    }
    override func keyDown(with event:NSEvent){switch event.keyCode{case 123,125:select((selected ?? 0)-1,commit:true);case 124,126:select((selected ?? -1)+1,commit:true);case 115:select(0,commit:true);case 119:select(levels.count-1,commit:true);default:super.keyDown(with:event)}}
    override func accessibilityPerformIncrement()->Bool{select((selected ?? -1)+1,commit:true);return true}
    override func accessibilityPerformDecrement()->Bool{select((selected ?? 0)-1,commit:true);return true}
}
