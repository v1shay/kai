import AppKit

/// History is loaded explicitly; scrolling never starts work on the main thread.
@MainActor final class ConversationScrollView:NSScrollView {}

@MainActor final class ConversationHistoryControl:NSView {
    var onLoad:(()->Void)?
    private let button=NSButton(title:"Load more",target:nil,action:nil)
    private let dots=CALayer()
    private var loading=false
    override init(frame:NSRect){
        super.init(frame:frame)
        wantsLayer=true
        button.isBordered=false;button.font=NSFont.systemFont(ofSize:9)
        button.contentTintColor=NSColor.white.withAlphaComponent(0.55)
        button.target=self;button.action=#selector(load);addSubview(button)
        dots.opacity=0;layer?.addSublayer(dots)
        for index in 0..<3 {
            let dot=CALayer();dot.bounds=CGRect(x:0,y:0,width:3,height:3)
            dot.position=CGPoint(x:CGFloat(index)*8+3,y:6);dot.cornerRadius=1.5
            dot.backgroundColor=NSColor.white.withAlphaComponent(0.65).cgColor;dots.addSublayer(dot)
        }
    }
    required init?(coder:NSCoder){fatalError()}
    @objc private func load(){guard !loading else{return};onLoad?()}
    override func layout(){
        super.layout();button.frame=CGRect(x:(bounds.width-80)/2,y:0,width:80,height:24)
        dots.frame=CGRect(x:(bounds.width-22)/2,y:6,width:22,height:12)
    }
    func update(hasMore:Bool,loading:Bool){
        isHidden = !hasMore && !loading
        guard self.loading != loading else{return};self.loading=loading
        button.isEnabled = !loading
        CATransaction.begin();CATransaction.setAnimationDuration(0.16)
        dots.opacity=loading ? 1:0
        CATransaction.commit()
        NSAnimationContext.runAnimationGroup { context in
            context.duration=0.16;button.animator().alphaValue=loading ? 0:1
        }
        for (index,dot) in (dots.sublayers ?? []).enumerated() {
            if loading {
                let jump=CAKeyframeAnimation(keyPath:"transform.translation.y")
                jump.values=[0, -4, 0, 0];jump.keyTimes=[0,0.3,0.6,1]
                jump.duration=0.85;jump.beginTime=CACurrentMediaTime()+Double(index)*0.12
                jump.repeatCount = .infinity;jump.timingFunctions=[CAMediaTimingFunction(name:.easeInEaseOut)]
                dot.add(jump,forKey:"loading")
            } else {dot.removeAnimation(forKey:"loading")}
        }
    }
}

@MainActor private final class ConversationRevealView:NSView {
    override func hitTest(_ point:NSPoint)->NSView? {nil}
}

@MainActor enum ConversationViewport {
    struct Anchor {let character:Int;let offset:CGFloat}
    static func anchor(_ text:NSTextView,in scroll:NSScrollView)->Anchor? {
        guard let manager=text.layoutManager,let container=text.textContainer,manager.numberOfGlyphs>0 else{return nil}
        let origin=scroll.contentView.bounds.origin,inset=text.textContainerOrigin
        let glyph=manager.glyphIndex(for:CGPoint(x:0,y:max(0,origin.y-inset.y)),in:container)
        let rect=manager.lineFragmentRect(forGlyphAt:min(glyph,manager.numberOfGlyphs-1),effectiveRange:nil)
        return Anchor(character:manager.characterIndexForGlyph(at:min(glyph,manager.numberOfGlyphs-1)),offset:origin.y-rect.minY-inset.y)
    }
    static func restore(_ anchor:Anchor,adding characters:Int=0,text:NSTextView,scroll:NSScrollView){
        guard let manager=text.layoutManager,manager.numberOfGlyphs>0 else{return}
        let character=min(max(0,anchor.character+characters),max(0,(text.string as NSString).length-1))
        let glyph=manager.glyphIndexForCharacter(at:character)
        let rect=manager.lineFragmentRect(forGlyphAt:glyph,effectiveRange:nil)
        scroll.contentView.scroll(to:CGPoint(x:0,y:max(0,rect.minY+text.textContainerOrigin.y+anchor.offset)))
        scroll.reflectScrolledClipView(scroll.contentView)
    }
    static func revealOlderText(_ text:NSTextView,characters:Int){
        guard characters>0,let manager=text.layoutManager,characters<(text.string as NSString).length else{return}
        let glyph=manager.glyphIndexForCharacter(at:characters)
        let end=manager.lineFragmentRect(forGlyphAt:glyph,effectiveRange:nil).minY+text.textContainerOrigin.y
        let cover=ConversationRevealView(frame:CGRect(x:0,y:text.textContainerOrigin.y,width:text.bounds.width,height:max(0,end-text.textContainerOrigin.y)))
        cover.wantsLayer=true;cover.layer?.backgroundColor=NSColor.black.cgColor
        text.addSubview(cover)
        NSAnimationContext.runAnimationGroup { context in
            context.duration=0.2;cover.animator().alphaValue=0
        } completionHandler: {cover.removeFromSuperview()}
    }
    static func layout(_ text:NSTextView,in scroll:NSScrollView){
        guard let container=text.textContainer,let manager=text.layoutManager else{return}
        let width=max(1,scroll.contentSize.width)
        if text.frame.width != width{text.setFrameSize(CGSize(width:width,height:text.frame.height))}
        manager.ensureLayout(for:container)
        let height=max(scroll.contentSize.height,ceil(manager.usedRect(for:container).maxY+text.textContainerInset.height*2))
        if text.frame.height != height{text.setFrameSize(CGSize(width:width,height:height))}
    }
    static func isAtBottom(_ text:NSTextView,in scroll:NSScrollView)->Bool {
        text.frame.height-scroll.contentView.bounds.maxY <= 24
    }
    static func bottom(_ text:NSTextView,in scroll:NSScrollView){
        layout(text,in:scroll)
        scroll.contentView.scroll(to:CGPoint(x:0,y:max(0,text.frame.height-scroll.contentSize.height)))
        scroll.reflectScrolledClipView(scroll.contentView)
    }
}
