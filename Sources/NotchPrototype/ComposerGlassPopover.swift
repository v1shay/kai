import AppKit

// Tile layout, deterministic fill mask, distance stagger and 1.4s pulse ported
// from MIT-licensed MonoCode's ModelPicker. See Licenses/Monocode.txt.
@MainActor final class EffortTileRow:NSControl {
    let value:String
    var commit:((String)->Void)?
    private let tiles=CALayer(),tint:NSColor
    private var tracking:NSTrackingArea?
    init(value:String,title:String,index:Int,count:Int,selected:Bool){
        self.value=value;tint=EffortAppearance.color(value,fraction:CGFloat(index)/CGFloat(max(1,count-1)))
        super.init(frame:CGRect(x:0,y:0,width:244,height:34));wantsLayer=true;layer?.cornerRadius=7;layer?.masksToBounds=true
        tiles.frame=bounds;layer?.addSublayer(tiles)
        let horizontal=CAGradientLayer();horizontal.frame=bounds;horizontal.startPoint=CGPoint(x:0,y:0.5);horizontal.endPoint=CGPoint(x:1,y:0.5);horizontal.colors=[NSColor.clear.cgColor,NSColor.white.cgColor,NSColor.white.cgColor,NSColor.clear.cgColor];horizontal.locations=[0,0.12,0.88,1];tiles.mask=horizontal
        for n in 0..<160 {
            let col=n%32,row=n/32,tile=CALayer();tile.frame=CGRect(x:CGFloat(col)*244/32,y:CGFloat(row)*34/5,width:244/32-0.7,height:34/5-0.7);tile.borderWidth=0.5;tile.borderColor=tint.withAlphaComponent(0.23).cgColor
            tile.name=(n*73+n*n*19+23)%101<65 ? "fill":nil;tile.opacity=row==0 || row==4 ? 0.2:row==1 || row==3 ? 0.65:1;tiles.addSublayer(tile)
        };tiles.opacity=0
        let label=CATextLayer();label.string=title;label.font=AppTypography.ui(ofSize:12,weight:.medium);label.fontSize=12;label.foregroundColor=tint.cgColor;label.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;label.frame=CGRect(x:12,y:9,width:210,height:17);layer?.addSublayer(label)
        if selected{let check=CATextLayer();check.string="✓";check.fontSize=12;check.foregroundColor=tint.cgColor;check.contentsScale=label.contentsScale;check.frame=CGRect(x:222,y:9,width:15,height:17);layer?.addSublayer(check);layer?.backgroundColor=NSColor.white.withAlphaComponent(0.06).cgColor}
        setAccessibilityLabel(title);setAccessibilityRole(.button)
    }
    required init?(coder:NSCoder){nil}
    override func updateTrackingAreas(){super.updateTrackingAreas();if let tracking{removeTrackingArea(tracking)};let area=NSTrackingArea(rect:bounds,options:[.mouseEnteredAndExited,.activeAlways],owner:self);addTrackingArea(area);tracking=area}
    override func mouseEntered(with event:NSEvent){
        CATransaction.begin();CATransaction.setDisableActions(true);tiles.opacity=1;CATransaction.commit()
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else{tiles.sublayers?.filter{$0.name=="fill"}.forEach{$0.backgroundColor=tint.withAlphaComponent(0.5).cgColor};return}
        let now=CACurrentMediaTime()
        for (n,tile) in (tiles.sublayers ?? []).enumerated() where tile.name=="fill"{
            let col=Double(n%32),row=Double(n/32),distance=hypot((col-15.5)/15.5,(row-2)/2)
            let pulse=CAKeyframeAnimation(keyPath:"backgroundColor");pulse.values=[NSColor.clear.cgColor,tint.cgColor,tint.withAlphaComponent(0.18).cgColor,NSColor.clear.cgColor];pulse.keyTimes=[0,0.26,0.52,1];pulse.duration=1.4;pulse.repeatCount=3;pulse.beginTime=now+distance*0.42;pulse.timingFunction=CAMediaTimingFunction(name:.easeOut);tile.add(pulse,forKey:"pulse")
        }
        let fade=CAKeyframeAnimation(keyPath:"opacity");fade.values=[1,1,0];fade.keyTimes=[0,0.92,1];fade.duration=5;fade.fillMode = .forwards;fade.isRemovedOnCompletion=false;tiles.add(fade,forKey:"feather")
    }
    override func mouseExited(with event:NSEvent){tiles.removeAllAnimations();tiles.opacity=0;tiles.sublayers?.forEach{$0.removeAllAnimations();$0.backgroundColor=nil}}
    override func mouseDown(with event:NSEvent){commit?(value)}
    override func accessibilityPerformPress()->Bool{commit?(value);return true}
}

@MainActor final class ComposerGlassPopover {
    private var panel:NSPanel?,monitor:Any?,globalMonitor:Any?
    var visible:Bool{panel?.isVisible == true}
    func dismiss(){if let monitor{NSEvent.removeMonitor(monitor)};if let globalMonitor{NSEvent.removeMonitor(globalMonitor)};monitor=nil;globalMonitor=nil;panel?.orderOut(nil);panel=nil}
    func show(title:String,rows:[NSView],at anchor:CGPoint){
        dismiss();let height=CGFloat(rows.count)*38+42,width:CGFloat=264
        let screen=NSScreen.screens.first{$0.frame.contains(anchor)}?.visibleFrame ?? NSScreen.main?.visibleFrame ?? CGRect(x:0,y:0,width:1200,height:800)
        let origin=CGPoint(x:min(max(screen.minX+8,anchor.x-30),screen.maxX-width-8),y:max(screen.minY+8,min(anchor.y-height-4,screen.maxY-height-8)))
        let panel=NSPanel(contentRect:CGRect(origin:origin,size:CGSize(width:width,height:height)),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false);panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=true;panel.level = .popUpMenu;panel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary]
        let glass=NSVisualEffectView(frame:CGRect(x:0,y:0,width:width,height:height));glass.material = .popover;glass.blendingMode = .behindWindow;glass.state = .active;glass.wantsLayer=true;glass.layer?.cornerRadius=14;glass.layer?.masksToBounds=true;glass.layer?.borderWidth=0.7;glass.layer?.borderColor=NSColor.white.withAlphaComponent(0.18).cgColor;panel.contentView=glass
        let heading=NSTextField(labelWithString:title);heading.font=AppTypography.ui(ofSize:11,weight:.medium);heading.textColor = .secondaryLabelColor;heading.frame=CGRect(x:15,y:height-29,width:width-30,height:18);glass.addSubview(heading)
        for (i,row) in rows.enumerated(){row.frame=CGRect(x:10,y:height-38-CGFloat(i+1)*38,width:244,height:34);glass.addSubview(row)}
        self.panel=panel;panel.alphaValue=0;panel.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup{c in c.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0:0.16;panel.animator().alphaValue=1}
        monitor=NSEvent.addLocalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown,.keyDown]){[weak self,weak panel] event in if event.type == .keyDown && event.keyCode==53{self?.dismiss();return nil};if event.type != .keyDown,event.window !== panel{self?.dismiss()};return event}
        globalMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown]){[weak self] _ in Task{@MainActor in self?.dismiss()}}
    }
}
