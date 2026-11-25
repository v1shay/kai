import AppKit

final class TextRevealPreferences {
    private let defaults:UserDefaults
    init(defaults:UserDefaults = .standard){self.defaults=defaults}
    var enabled:Bool {
        get{defaults.bool(forKey:"morphingConversationText")}
        set{defaults.set(newValue,forKey:"morphingConversationText")}
    }
}

enum TextRevealChange {
    /// Diff display strings, after markdown, citations and attachments are formatted.
    /// Never split a composed character (emoji, combining marks, surrogate pairs).
    static func suffix(old:String,new:String)->NSRange? {
        guard old != new else{return nil}
        var prefix=0
        for (a,b) in zip(old,new){guard a == b else{break};prefix += b.utf16.count}
        let length=(new as NSString).length-prefix
        return length>0 ? NSRange(location:prefix,length:length):nil
    }
    static func visible(_ ranges:[NSRange],in viewport:NSRange,budget:Int=512)->[NSRange]{
        var remaining=max(0,budget),result=[NSRange]()
        for range in ranges {
            let visible=NSIntersectionRange(range,viewport)
            guard visible.length>0,remaining>0 else{continue}
            let count=min(visible.length,remaining)
            result.append(NSRange(location:visible.location,length:count));remaining -= count
        }
        return result
    }
}

/// Tiny, finite bitmap sequences run in Core Animation. No animation timer,
/// text mutations, per-frame layout, blur, or per-character layers.
@MainActor final class ConversationTextReveal {
    static let duration=0.28,maximumTiles=24,maximumGlyphs=512
    private weak var text:NSTextView?,manager:NSLayoutManager?
    private var maskedCharacters=[NSRange]()
    private var tiles=[CALayer](),cleanup:DispatchWorkItem?
    private var generation=UUID()
    private(set) var snapshotCount=0
    var activeTileCount:Int{tiles.count}

    func finish(){
        guard !tiles.isEmpty else{return}
        generation=UUID();cleanup?.cancel();cleanup=nil
        (manager as? InteractionLayoutManager)?.revealGlyphRanges=[]
        if let manager{for range in maskedCharacters{let clipped=NSIntersectionRange(range,NSRange(location:0,length:manager.textStorage?.length ?? 0));if clipped.length>0{manager.removeTemporaryAttribute(.foregroundColor,forCharacterRange:clipped)}}};maskedCharacters=[]
        tiles.forEach{$0.removeAllAnimations();$0.removeFromSuperlayer()};tiles=[]
        text?.needsDisplay=true;text=nil;manager=nil
    }

    func reveal(_ ranges:[NSRange],in text:NSTextView,loading:Bool){
        finish();snapshotCount=0
        guard !ranges.isEmpty,!text.isHiddenOrHasHiddenAncestor,!NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,
              let manager=text.layoutManager,
              let container=text.textContainer,let storage=text.textStorage,storage.length>0 else{return}
        let origin=text.textContainerOrigin
        let visible=text.visibleRect.offsetBy(dx:-origin.x,dy:-origin.y)
        guard visible.width>0,visible.height>0 else{return}
        let viewport=manager.glyphRange(forBoundingRect:visible,in:container)
        let characters=manager.characterRange(forGlyphRange:viewport,actualGlyphRange:nil)
        let changes=TextRevealChange.visible(ranges,in:characters,budget:Self.maximumGlyphs)
        guard !changes.isEmpty else{return}
        text.wantsLayer=true
        let scale=text.window?.backingScaleFactor ?? NSScreen.main?.backingScaleFactor ?? 2
        var hidden=[NSRange]()
        CATransaction.begin();CATransaction.setDisableActions(true)
        for change in changes {
            let glyphs=manager.glyphRange(forCharacterRange:change,actualCharacterRange:nil)
            var cursor=glyphs.location
            while cursor<NSMaxRange(glyphs),tiles.count<Self.maximumTiles {
                var line=NSRange()
                _=manager.lineFragmentRect(forGlyphAt:cursor,effectiveRange:&line)
                let count=min(64,min(NSMaxRange(line),NSMaxRange(glyphs))-cursor)
                guard count>0 else{break}
                let chunk=NSRange(location:cursor,length:count);cursor += count
                let ink=manager.boundingRect(forGlyphRange:chunk,in:container).insetBy(dx:-1,dy:-1)
                // Keep textures on backing-pixel boundaries. Fractional resampling
                // makes the last bitmap look softer than the native text beneath it.
                let x=floor(ink.minX*scale)/scale,y=floor(ink.minY*scale)/scale
                let rect=CGRect(x:x,y:y,width:ceil(ink.maxX*scale)/scale-x,height:ceil(ink.maxY*scale)/scale-y)
                guard rect.intersects(visible),rect.width>1,rect.height>1 else{continue}
                // Keep oversized attachments and unusually tall lines on the native path.
                guard rect.width*rect.height*scale*scale<=512*128 else{continue}
                guard let final=bitmap(rect:rect,scale:scale,draw:{
                    manager.drawGlyphs(forGlyphRange:chunk,at:CGPoint(x:-rect.minX,y:-rect.minY))
                }) else{continue}
                var frames=[CGImage]()
                for phase in 0..<3 {
                    if let ghost=bitmap(rect:rect,scale:scale,draw:{
                        Self.drawGhost(chunk,phase:phase,manager:manager,storage:storage,rect:rect)
                    }){frames.append(ghost);snapshotCount += 1}
                }
                frames.append(final);snapshotCount += 1
                let tile=CALayer();tile.frame=rect.offsetBy(dx:origin.x,dy:origin.y)
                tile.contents=final;tile.contentsScale=scale;tile.contentsGravity = .resize
                tile.isGeometryFlipped=true
                text.layer?.addSublayer(tile);tiles.append(tile);hidden.append(chunk)
                let sequence=CAKeyframeAnimation(keyPath:"contents")
                sequence.values=frames;sequence.keyTimes=frames.indices.map{NSNumber(value:Double($0)/Double(max(1,frames.count-1))*0.8)}
                sequence.calculationMode = .discrete;sequence.duration=Self.duration
                let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=0.25;fade.toValue=1;fade.duration=Self.duration
                let animation=CAAnimationGroup();animation.animations=[sequence,fade]
                animation.duration=Self.duration
                animation.beginTime=CACurrentMediaTime()+(loading ? min(0.1,Double(tiles.count-1)*0.008):0)
                animation.fillMode = .backwards
                tile.addKai(animation,forKey:"textDecode")
            }
        }
        // The real string is always complete for copy, links and accessibility.
        if let custom=manager as? InteractionLayoutManager{custom.revealGlyphRanges=hidden}else{
            maskedCharacters=hidden.map{manager.characterRange(forGlyphRange:$0,actualGlyphRange:nil)}
            for range in maskedCharacters{manager.addTemporaryAttribute(.foregroundColor,value:NSColor.clear,forCharacterRange:range)}
        }
        CATransaction.commit()
        guard !tiles.isEmpty else{return}
        self.text=text;self.manager=manager;text.needsDisplay=true
        let token=UUID();generation=token
        let work=DispatchWorkItem{[weak self] in guard let self,self.generation == token else{return};self.finish()}
        cleanup=work;DispatchQueue.main.asyncAfter(deadline:.now()+Self.duration+0.12,execute:work)
    }

    private func bitmap(rect:CGRect,scale:CGFloat,draw:()->Void)->CGImage? {
        let width=Int(ceil(rect.width*scale)),height=Int(ceil(rect.height*scale))
        guard let bitmap=NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:width,pixelsHigh:height,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0),
              let graphics=NSGraphicsContext(bitmapImageRep:bitmap) else{return nil}
        NSGraphicsContext.saveGraphicsState();defer{NSGraphicsContext.restoreGraphicsState()}
        let context=graphics.cgContext
        context.translateBy(x:0,y:CGFloat(height));context.scaleBy(x:scale,y:-scale)
        NSGraphicsContext.current=NSGraphicsContext(cgContext:context,flipped:true)
        draw();return bitmap.cgImage
    }

    private static func drawGhost(_ glyphs:NSRange,phase:Int,manager:NSLayoutManager,storage:NSTextStorage,rect:CGRect){
        let string=storage.string as NSString
        for glyph in glyphs.location..<NSMaxRange(glyphs) {
            let character=manager.characterIndexForGlyph(at:glyph)
            guard character<string.length else{continue}
            let code=string.character(at:character)
            // Keep emoji, ligatures and attachments native; scramble ASCII letters only.
            guard (65...90).contains(code) || (97...122).contains(code) else{
                manager.drawGlyphs(forGlyphRange:NSRange(location:glyph,length:1),at:CGPoint(x:-rect.minX,y:-rect.minY));continue
            }
            let seed=(character &* 31 &+ phase &* 17) & 255
            if seed%5>phase+1{continue}
            let actual=String(UnicodeScalar(code)!)
            let neighbor=min(string.length-1,character+(phase == 0 ? 2:1))
            let neighborCode=string.character(at:neighbor)
            let replacement=(97...122).contains(neighborCode) ? String(UnicodeScalar(neighborCode)!):actual
            let attributes=storage.attributes(at:character,effectiveRange:nil)
            let font=attributes[.font] as? NSFont ?? NSFont.systemFont(ofSize:9)
            let color=attributes[.foregroundColor] as? NSColor ?? .white
            let line=manager.lineFragmentRect(forGlyphAt:glyph,effectiveRange:nil),point=manager.location(forGlyphAt:glyph)
            let position=CGPoint(x:line.minX+point.x-rect.minX,y:line.minY+point.y-font.ascender-rect.minY)
            (phase == 2 && seed%3 == 0 ? actual:replacement).draw(at:position,withAttributes:[.font:font,.foregroundColor:color.withAlphaComponent(0.5+Double(phase)*0.15)])
        }
    }
}
