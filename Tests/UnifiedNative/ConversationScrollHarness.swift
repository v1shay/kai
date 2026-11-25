import AppKit

private final class ScrollHarnessBackground:NSView {
    override func draw(_ dirtyRect:NSRect){NSColor.black.setFill();bounds.fill()}
}
@main struct ConversationScrollHarness {
    @MainActor static func main() throws {
        _=NSApplication.shared
        let destination=CommandLine.arguments.dropFirst().first{!$0.hasPrefix("--")}
        let output=URL(fileURLWithPath:destination ?? NSTemporaryDirectory()+"kai-history-scroll-"+UUID().uuidString,isDirectory:true)
        try FileManager.default.createDirectory(at:output,withIntermediateDirectories:true)
        var rows=[[String:Any]](),failures=[String]()
        if !ConversationScrollView().autohidesScrollers{failures.append("default real history constructor keeps an empty track")}
        let appearances:[NSAppearance.Name]=[.aqua,.darkAqua,.accessibilityHighContrastAqua,.accessibilityHighContrastDarkAqua]
        for appearance in appearances {for style:NSScroller.Style in [.legacy,.overlay] {for overflow in [false,true] {
            let name="\(appearance.rawValue)-\(style.rawValue)-\(overflow ? "overflow":"empty")"
            let window=NSWindow(contentRect:NSRect(x:0,y:0,width:220,height:120),styleMask:[.borderless],backing:.buffered,defer:false)
            window.isReleasedWhenClosed=false;window.appearance=NSAppearance(named:appearance)
            defer{window.orderOut(nil);window.close()}
            let root=ScrollHarnessBackground(frame:NSRect(x:0,y:0,width:220,height:120));window.contentView=root
            // Same history widget/configuration as App.swift. Style changes are
            // local simulations; no user defaults/accessibility options change.
            let scroll=ConversationScrollView(frame:root.bounds),text=NSTextView(frame:NSRect(x:0,y:0,width:220,height:overflow ? 600:120))
            scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.hasHorizontalScroller=false
            scroll.scrollerStyle=style;scroll.borderType = .noBorder;scroll.contentView.drawsBackground=false
            text.drawsBackground=false;text.isEditable=false;text.isSelectable=true;text.textColor = .white;text.font = .systemFont(ofSize:9);text.textContainerInset=NSSize(width:3,height:24)
            if overflow{text.string=Array(repeating:"History reply",count:50).joined(separator:"\n")}
            scroll.documentView=text;root.addSubview(scroll);scroll.tile();root.layoutSubtreeIfNeeded();scroll.reflectScrolledClipView(scroll.contentView)
            let scroller=scroll.verticalScroller!
            let bitmap=root.bitmapImageRepForCachingDisplay(in:root.bounds)!;root.cacheDisplay(in:root.bounds,to:bitmap)
            var brightPixels=0
            for y in 0..<bitmap.pixelsHigh {for x in 0..<bitmap.pixelsWide {
                if let color=bitmap.colorAt(x:x,y:y)?.usingColorSpace(.sRGB),color.alphaComponent>0.9,color.redComponent>0.94,color.greenComponent>0.94,color.blueComponent>0.94{brightPixels+=1}
            }}
            var row:[String:Any]=["name":name,"appearance":scroll.effectiveAppearance.name.rawValue,"requestedAppearance":appearance.rawValue,"windowEffectiveAppearance":window.effectiveAppearance.name.rawValue,"localAppearanceOverride":scroll.appearance?.name.rawValue ?? "none","style":scroll.scrollerStyle.rawValue,"preferredStyle":NSScroller.preferredScrollerStyle.rawValue,"autohides":scroll.autohidesScrollers,"documentHeight":text.frame.height,"clipHeight":scroll.contentView.bounds.height,"scrollerHidden":scroller.isHidden,"scrollerWidth":scroller.frame.width,"scrollerHeight":scroller.frame.height,"knobProportion":scroller.knobProportion,"brightPixels":brightPixels,"windowVisible":window.isVisible]
            func require(_ condition:Bool,_ message:String){if !condition{failures.append(name+": "+message)}}
            require(scroll.autohidesScrollers,"history does not hide an unusable empty scroller")
            require(scroll.scrollerStyle==style,"system-style simulation was overridden")
            require(scroll.appearance == nil,"history installed a forced appearance instead of inheriting the system/parent appearance")
            if overflow {
                require(!scroller.isHidden,"overflow history lost its native scrollbar")
                require(scroller.knobProportion>0 && scroller.knobProportion<1,"overflow thumb range is invalid")
                scroll.contentView.scroll(to:NSPoint(x:0,y:max(0,text.frame.height-scroll.contentSize.height)))
                scroll.reflectScrolledClipView(scroll.contentView)
                row["bottomOriginY"]=scroll.contentView.bounds.origin.y
                require(scroll.contentView.bounds.origin.y>0,"overflow document cannot scroll to the bottom")
                text.string="";text.setFrameSize(NSSize(width:scroll.contentSize.width,height:scroll.contentSize.height));scroll.tile();scroll.reflectScrolledClipView(scroll.contentView)
                for _ in 0..<3{scroll.tile();root.layoutSubtreeIfNeeded();scroll.reflectScrolledClipView(scroll.contentView)}
                row["hiddenAfterClear"]=scroll.verticalScroller?.isHidden ?? true
                require(scroll.verticalScroller?.isHidden ?? true,"clearing a long history left an empty track")
            } else {
                require(text.frame.height<=scroll.contentView.bounds.height+0.5,"empty fixture unexpectedly overflows")
                require(scroller.isHidden,"empty history displays a native track")
                require(brightPixels==0,"empty dark history contains an opaque bright strip")
                text.string=Array(repeating:"History reply",count:50).joined(separator:"\n");text.setFrameSize(NSSize(width:scroll.contentSize.width,height:600));scroll.tile();scroll.reflectScrolledClipView(scroll.contentView)
                for _ in 0..<3{scroll.tile();root.layoutSubtreeIfNeeded();scroll.reflectScrolledClipView(scroll.contentView)}
                row["visibleAfterGrowth"] = !(scroll.verticalScroller?.isHidden ?? true)
                require(!(scroll.verticalScroller?.isHidden ?? true),"growing history failed to restore its scrollbar")
            }
            try bitmap.representation(using:.png,properties:[:])!.write(to:output.appendingPathComponent(name+".png"));rows.append(row)
        }}}
        let report:[String:Any]=["cases":rows,"failures":failures,"preferredStyle":NSScroller.preferredScrollerStyle.rawValue,"increaseContrast":NSWorkspace.shared.accessibilityDisplayShouldIncreaseContrast,"reduceMotion":NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,"scope":"Offscreen local views only; no system preferences or permissions changed"]
        try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:output.appendingPathComponent("history-scroll.json"))
        if !failures.isEmpty {for failure in failures{fputs(failure+"\n",stderr)};exit(1)}
        print("CONVERSATION_SCROLL_HARNESS_OK cases=\(rows.count) empty/overflow legacy/overlay standard/high-contrast")
    }
}
