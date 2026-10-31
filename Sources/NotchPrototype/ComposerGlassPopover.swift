import AppKit

// Tile layout, deterministic fill mask, distance stagger and 1.4s pulse ported
// from MIT-licensed MonoCode's ModelPicker. See Licenses/Monocode.txt.
// A five-second burst is compositor-driven, then becomes completely idle.
@MainActor final class EffortTileLayer:CALayer {
    private var cells=[CALayer]()
    private let feather=CAGradientLayer()
    private var tint=NSColor.white
    private(set) var previewing=false
    override init(){
        super.init();masksToBounds=true;cornerRadius=5;opacity=0
        feather.startPoint=CGPoint(x:0,y:0.5);feather.endPoint=CGPoint(x:1,y:0.5);feather.colors=[NSColor.clear.cgColor,NSColor.white.cgColor,NSColor.white.cgColor,NSColor.clear.cgColor];feather.locations=[0,0.12,0.88,1];mask=feather
        for n in 0..<160{let tile=CALayer();tile.name=(n*73+n*n*19+23)%101<65 ? "fill":nil;tile.borderWidth=0.45;addSublayer(tile);cells.append(tile)}
    }
    override init(layer:Any){super.init(layer:layer)}
    required init?(coder:NSCoder){nil}
    func configure(size:CGSize,tint:NSColor){
        self.tint=tint;CATransaction.begin();CATransaction.setDisableActions(true);bounds=CGRect(origin:.zero,size:size);feather.frame=bounds
        for(n,tile) in cells.enumerated(){let row=n/32;tile.frame=CGRect(x:CGFloat(n%32)*size.width/32,y:CGFloat(row)*size.height/5,width:max(0,size.width/32-0.55),height:max(0,size.height/5-0.55));tile.borderColor=tint.withAlphaComponent(0.23).cgColor;tile.opacity=row==0 || row==4 ? 0.2:row==1 || row==3 ? 0.65:1}
        CATransaction.commit()
    }
    func play(restart:Bool=false){
        guard restart || !previewing else{return};previewing=true;removeAllAnimations();CATransaction.begin();CATransaction.setDisableActions(true);opacity=1;CATransaction.commit()
        for tile in cells{tile.removeAllAnimations();tile.backgroundColor=nil}
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else{cells.filter{$0.name=="fill"}.forEach{$0.backgroundColor=tint.withAlphaComponent(0.5).cgColor};return}
        let now=CACurrentMediaTime()
        for(n,tile) in cells.enumerated() where tile.name=="fill"{
            let distance=hypot((Double(n%32)-15.5)/15.5,(Double(n/32)-2)/2)
            let pulse=CAKeyframeAnimation(keyPath:"backgroundColor");pulse.values=[NSColor.clear.cgColor,tint.cgColor,tint.withAlphaComponent(0.18).cgColor,NSColor.clear.cgColor];pulse.keyTimes=[0,0.26,0.52,1];pulse.duration=1.4;pulse.repeatCount=3;pulse.beginTime=now+distance*0.42;pulse.timingFunction=CAMediaTimingFunction(name:.easeOut);tile.add(pulse,forKey:"pulse")
        }
        let fade=CAKeyframeAnimation(keyPath:"opacity");fade.values=[1,1,0];fade.keyTimes=[0,0.92,1];fade.duration=5;fade.fillMode = .forwards;fade.isRemovedOnCompletion=false;add(fade,forKey:"feather")
    }
    func stop(){previewing=false;removeAllAnimations();opacity=0;for tile in cells{tile.removeAllAnimations();tile.backgroundColor=nil}}
}

@MainActor final class EffortTileRow:NSControl {
    let value:String,selected:Bool
    var commit:((String)->Void)?,onPreview:((String,Bool)->Void)?
    private let tiles=EffortTileLayer(),caption=CATextLayer(),check=CATextLayer()
    private var rowTint=NSColor.white
    private var tracking:NSTrackingArea?
    init(value:String,title:String,index:Int,count:Int,selected:Bool){
        self.value=value;self.selected=selected
        let tint=EffortAppearance.color(value,fraction:CGFloat(index)/CGFloat(max(1,count-1)))
        super.init(frame:CGRect(x:0,y:0,width:244,height:34));wantsLayer=true;layer?.cornerRadius=7;layer?.masksToBounds=true
        tiles.frame=bounds;tiles.configure(size:bounds.size,tint:tint);layer?.addSublayer(tiles)
        rowTint=tint;let label=caption;label.string=title;label.font=AppTypography.ui(ofSize:12,weight:.medium);label.fontSize=12;label.foregroundColor=tint.cgColor;label.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;label.frame=CGRect(x:12,y:9,width:210,height:17);layer?.addSublayer(label)
        if selected{check.string="✓";check.fontSize=12;check.foregroundColor=tint.cgColor;check.contentsScale=label.contentsScale;check.frame=CGRect(x:222,y:9,width:15,height:17);layer?.addSublayer(check);layer?.backgroundColor=NSColor.white.withAlphaComponent(0.06).cgColor}
        setAccessibilityLabel(title);setAccessibilityRole(.button)
    }
    required init?(coder:NSCoder){nil}
    override func layout(){
        super.layout();CATransaction.begin();CATransaction.setDisableActions(true)
        tiles.frame=bounds;tiles.configure(size:bounds.size,tint:rowTint)
        caption.frame=CGRect(x:12,y:9,width:max(0,bounds.width-42),height:17)
        check.frame=CGRect(x:bounds.width-22,y:9,width:15,height:17);CATransaction.commit()
    }
    func setPreview(_ active:Bool){if active{tiles.play()}else{tiles.stop()}}
    override func updateTrackingAreas(){super.updateTrackingAreas();if let tracking{removeTrackingArea(tracking)};let area=NSTrackingArea(rect:bounds,options:[.mouseEnteredAndExited,.activeAlways],owner:self);addTrackingArea(area);tracking=area}
    override func mouseEntered(with event:NSEvent){if let onPreview{onPreview(value,true)}else{setPreview(true)}}
    override func mouseExited(with event:NSEvent){if let onPreview{onPreview(value,false)}else{setPreview(selected)}}
    override func mouseDown(with event:NSEvent){commit?(value)}
    override func accessibilityPerformPress()->Bool{commit?(value);return true}
}

@MainActor final class ComposerGlassPopover {
    private var panel:NSPanel?,monitor:Any?,globalMonitor:Any?
    private var key:String?
    private(set) var generation=0
    private var effortRows=[EffortTileRow](),hoveredEffort:String?
    private func preview(_ value:String,_ hovering:Bool){
        if hovering{hoveredEffort=value}else if hoveredEffort==value{hoveredEffort=nil}else{return}
        let target=hoveredEffort ?? effortRows.first(where:{$0.selected})?.value
        for row in effortRows{row.setPreview(row.value==target)}
    }
    var visible:Bool{panel?.isVisible == true}
    func dismiss(ifUnchanged token:Int){if generation==token{dismiss()}}
    func dismiss(){generation &+= 1;key=nil;effortRows.forEach{$0.setPreview(false);$0.onPreview=nil};effortRows=[];hoveredEffort=nil;if let monitor{NSEvent.removeMonitor(monitor)};if let globalMonitor{NSEvent.removeMonitor(globalMonitor)};monitor=nil;globalMonitor=nil;panel?.orderOut(nil);panel=nil}
    func show(key:String,title:String,rows:[NSView],at anchor:CGPoint){
        if visible,self.key==key{dismiss();return}
        dismiss();self.key=key;let height=CGFloat(rows.count)*38+42,width:CGFloat=264
        let screen=NSScreen.screens.first{$0.frame.contains(anchor)}?.visibleFrame ?? NSScreen.main?.visibleFrame ?? CGRect(x:0,y:0,width:1200,height:800)
        let origin=CGPoint(x:min(max(screen.minX+8,anchor.x-30),screen.maxX-width-8),y:max(screen.minY+8,min(anchor.y-height-4,screen.maxY-height-8)))
        let panel=NSPanel(contentRect:CGRect(origin:origin,size:CGSize(width:width,height:height)),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false);panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=true;panel.level = .popUpMenu;panel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary]
        let glass=NSVisualEffectView(frame:CGRect(x:0,y:0,width:width,height:height));glass.material = .popover;glass.blendingMode = .behindWindow;glass.state = .active;glass.wantsLayer=true;glass.layer?.cornerRadius=14;glass.layer?.masksToBounds=true;glass.layer?.borderWidth=0.7;glass.layer?.borderColor=NSColor.white.withAlphaComponent(0.18).cgColor;panel.contentView=glass
        let heading=NSTextField(labelWithString:title);heading.font=AppTypography.ui(ofSize:11,weight:.medium);heading.textColor = .secondaryLabelColor;heading.frame=CGRect(x:15,y:height-29,width:width-30,height:18);glass.addSubview(heading)
        for (i,row) in rows.enumerated(){row.frame=CGRect(x:10,y:height-38-CGFloat(i+1)*38,width:244,height:34);glass.addSubview(row)}
        self.panel=panel;panel.alphaValue=0;panel.orderFrontRegardless()
        effortRows=rows.compactMap{$0 as? EffortTileRow};for row in effortRows{row.onPreview={[weak self] value,hovering in self?.preview(value,hovering)};row.setPreview(row.selected)}
        NSAnimationContext.runAnimationGroup{c in c.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0:0.16;panel.animator().alphaValue=1}
        monitor=NSEvent.addLocalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown,.keyDown]){[weak self,weak panel] event in if event.type == .keyDown && event.keyCode==53{self?.dismiss();return nil};if event.type != .keyDown,event.window !== panel{DispatchQueue.main.async{[weak self,weak panel] in guard let self,let panel,self.panel === panel else{return};self.dismiss()}};return event}
        globalMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown]){[weak self] _ in Task{@MainActor in self?.dismiss()}}
    }
}
