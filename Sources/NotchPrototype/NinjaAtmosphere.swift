import AppKit
import QuartzCore

/// A transparent, non-interactive scene. Motion lives on the render server;
/// no display link, polling, GPU readback, or conversation rerender timer.
@MainActor final class NinjaAtmosphereView:NSView {
    enum Phase:String {case idle,working,finished}
    private let field=CALayer(),grid=CALayer(),hoverTile=CALayer(),actor=CALayer(),portrait=CALayer(),caption=CATextLayer()
    private var tiles=[CALayer](),observers=[NSObjectProtocol]()
    private var identity="",style="fire",phase=Phase.idle,motion=true,paused=false
    private var rasterKey="",sceneKey="",lastPointer:CFTimeInterval=0
    private var tracking:NSTrackingArea?
    private var lastGreeting:CFTimeInterval=0
    private static var portraits=[String:[CGImage]]()
    override var isFlipped:Bool{true}
    override func hitTest(_ point:NSPoint)->NSView?{nil}
    override init(frame:NSRect){
        super.init(frame:frame);wantsLayer=true;layer?.masksToBounds=true
        layer?.addSublayer(field);field.addSublayer(grid);field.addSublayer(hoverTile);hoverTile.opacity=0;hoverTile.cornerRadius=0.8;layer?.addSublayer(actor);actor.addSublayer(portrait);actor.addSublayer(caption)
        caption.font=NSFont.monospacedSystemFont(ofSize:8,weight:.medium);caption.fontSize=8;caption.alignmentMode = .center;caption.opacity=0
        for _ in 0..<28 {let tile=CALayer();tile.cornerRadius=0.8;tile.borderWidth=0.5;field.addSublayer(tile);tiles.append(tile)}
        for name in [NSWindow.didBecomeKeyNotification,NSWindow.didResignKeyNotification,NSWindow.didChangeOcclusionStateNotification,NSApplication.didBecomeActiveNotification,NSApplication.didResignActiveNotification]{
            observers.append(NotificationCenter.default.addObserver(forName:name,object:nil,queue:.main){[weak self] _ in MainActor.assumeIsolated{self?.updatePlayback()}})
        }
    }
    required init?(coder:NSCoder){fatalError("init(coder:) has not been implemented")}
    override func viewDidMoveToWindow(){super.viewDidMoveToWindow();rasterKey="";needsLayout=true;updatePlayback()}
    override func viewDidChangeBackingProperties(){super.viewDidChangeBackingProperties();rasterKey="";needsLayout=true}
    override func updateTrackingAreas(){
        if let tracking{removeTrackingArea(tracking)}
        let area=NSTrackingArea(rect:bounds,options:[.mouseMoved,.mouseEnteredAndExited,.activeInKeyWindow,.inVisibleRect],owner:self,userInfo:nil);addTrackingArea(area);tracking=area;super.updateTrackingAreas()
    }
    static func color(_ style:String)->NSColor {
        let hex:[String:UInt32]=["fire":0xef6758,"lightning":0x619dff,"earth":0xa5acba,"ice":0xc2edf4,"energy":0x74d6a0,"water":0x60ccd7]
        let c=hex[style] ?? 0xef6758
        return NSColor(srgbRed:CGFloat((c>>16)&255)/255,green:CGFloat((c>>8)&255)/255,blue:CGFloat(c&255)/255,alpha:1)
    }
    func configure(identity:String,style:String,working:Bool,motion:Bool,visible:Bool){
        let opening=self.identity != identity || (isHidden && visible)
        let completed=self.phase == .working && !working && !opening
        let starting=working && self.phase != .working
        let changed=self.style != style
        self.identity=identity;self.style=style;self.motion=motion && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        phase=working ? .working:(completed ? .finished:(opening ? .idle:phase == .finished ? .finished:.idle))
        isHidden = !visible
        if changed{rasterKey=""}
        if !visible{sceneKey="";updatePlayback();return}
        needsLayout=true;layoutSubtreeIfNeeded()
        if opening || changed {sceneKey=""}
        animateScene()
        if visible && (opening || completed || starting) {say(completed ? "ALL SET":working ? "ON IT":"HELLO",opening:opening)}
        updatePlayback()
    }
    override func layout(){
        super.layout();guard !isHidden,bounds.width>0,bounds.height>0 else{return}
        CATransaction.begin();CATransaction.setDisableActions(true)
        let height=min(210,max(96,bounds.height*0.34));field.frame=CGRect(x:0,y:0,width:bounds.width,height:height)
        let scale=window?.backingScaleFactor ?? 2
        let key="\(Int(bounds.width*scale)):\(Int(height*scale)):\(style)"
        if key != rasterKey {rasterKey=key;grid.frame=field.bounds;grid.contentsScale=scale;grid.contents=makeGrid(size:field.bounds.size,scale:scale);sceneKey=""}
        portrait.frame=CGRect(x:-16,y:-19,width:32,height:38);portrait.contentsGravity = .resizeAspect;portrait.magnificationFilter = .nearest
        if let frame=frames().first{portrait.contents=frame}
        caption.frame=CGRect(x:-42,y:-39,width:84,height:14);caption.contentsScale=scale;caption.foregroundColor=Self.color(style).withAlphaComponent(0.85).cgColor
        for (i,tile) in tiles.enumerated(){
            let x=CGFloat((i*137+61)%997)/997*max(1,bounds.width-14)+7
            let y=CGFloat((i*53+19)%173)/173*(height-28)+6
            tile.frame=CGRect(x:x,y:y,width:i%5==0 ? 5:3,height:i%5==0 ? 5:3)
            tile.backgroundColor=Self.color(style).withAlphaComponent(0.11).cgColor
            tile.borderColor=Self.color(style).withAlphaComponent(0.15).cgColor
            tile.opacity=Float(pow(1-y/height,1.7))*0.7
        }
        CATransaction.commit();animateScene()
    }
    private func makeGrid(size:CGSize,scale:CGFloat)->CGImage?{
        guard let c=CGContext(data:nil,width:max(1,Int(size.width*scale)),height:max(1,Int(size.height*scale)),bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else{return nil}
        c.scaleBy(x:scale,y:scale);c.translateBy(x:0,y:size.height);c.scaleBy(x:1,y:-1)
        for row in 0...Int(size.height/7){for col in 0...Int(size.width/7){
            let fade=pow(max(0,1-CGFloat(row)*7/size.height),2.2)
            let rect=CGRect(x:CGFloat(col)*7+0.5,y:CGFloat(row)*7+0.5,width:5,height:5)
            c.setStrokeColor(Self.color(style).withAlphaComponent(0.055*fade).cgColor);c.setLineWidth(0.45);c.stroke(rect)
            let hash=(col*73+row*137+col*row*19)%101
            if hash<8 {c.setFillColor(Self.color(style).withAlphaComponent((hash<2 ? 0.12:0.045)*fade).cgColor);c.fill(rect.insetBy(dx:0.5,dy:0.5))}
        }}
        return c.makeImage()
    }
    private func frames()->[CGImage]{
        if let result=Self.portraits[style]{return result}
        let color=Self.color(style)
        let result=(0..<4).compactMap{index->CGImage? in
            guard let url=Bundle.main.resourceURL?.appendingPathComponent("providers/ninja-walk-\(index).svg"),let image=NSImage(contentsOf:url),let cg=image.cgImage(forProposedRect:nil,context:nil,hints:nil),let c=CGContext(data:nil,width:48,height:56,bitsPerComponent:8,bytesPerRow:192,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else{return nil}
            c.clip(to:CGRect(x:0,y:0,width:48,height:56),mask:cg);c.setFillColor(color.cgColor);c.fill(CGRect(x:0,y:0,width:48,height:56));return c.makeImage()
        };Self.portraits[style]=result;return result
    }
    private func animateScene(){
        guard bounds.width>0 else{return}
        let key="\(identity)/\(style)/\(phase.rawValue)/\(motion)/\(rasterKey)"
        guard key != sceneKey else{return};sceneKey=key
        let current=actor.presentation()?.position ?? CGPoint(x:bounds.width*0.22,y:58)
        actor.removeAllAnimations();portrait.removeAllAnimations();field.removeAllAnimations();tiles.forEach{$0.removeAllAnimations()}
        CATransaction.begin();CATransaction.setDisableActions(true);actor.position=CGPoint(x:bounds.width*0.24,y:60);actor.opacity=0.8;field.opacity=1;CATransaction.commit()
        guard motion else{return}
        let working=phase == .working
        let travel=CAKeyframeAnimation(keyPath:"position")
        let h=field.bounds.height
        let start=CGPoint(x:min(max(24,current.x),bounds.width-24),y:min(max(28,current.y),h-32))
        travel.values=[start,CGPoint(x:bounds.width*0.43,y:h*0.32),CGPoint(x:bounds.width*0.76,y:h*0.54),CGPoint(x:bounds.width*0.62,y:h*0.30),CGPoint(x:bounds.width*0.32,y:h*0.60),start].map{NSValue(point:$0)}
        travel.duration=working ? 15:32;travel.repeatCount = .infinity;travel.calculationMode = .cubicPaced;actor.add(travel,forKey:"float")
        let bob=CAKeyframeAnimation(keyPath:"transform.translation.y");bob.values=[0,-3,0,2,0];bob.duration=working ? 2.4:4.8;bob.repeatCount = .infinity;portrait.add(bob,forKey:"breathe")
        let images=frames();if working,images.count>1{let gait=CAKeyframeAnimation(keyPath:"contents");gait.values=images;gait.duration=0.65;gait.repeatCount = .infinity;gait.calculationMode = .discrete;portrait.add(gait,forKey:"poses")}
        for (i,tile) in tiles.enumerated(){
            let wave=CAKeyframeAnimation(keyPath:"opacity");let base=tile.opacity
            wave.values=[base*0.35,base*(working ? 1.9:1.1),base*0.35];wave.duration=working ? 3.6:8.5;wave.beginTime=tile.convertTime(CACurrentMediaTime(),from:nil)+Double(i%11)*0.19;wave.repeatCount = .infinity;wave.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);tile.add(wave,forKey:"glimmer")
            let drift=CABasicAnimation(keyPath:"transform.translation.y");drift.fromValue=2;drift.toValue = -3;drift.autoreverses=true;drift.duration=Double(4+i%5);drift.repeatCount = .infinity;tile.add(drift,forKey:"drift")
        }
        if phase == .finished{let hop=CAKeyframeAnimation(keyPath:"transform.translation.y");hop.values=[0,-10,0,-4,0];hop.duration=0.8;portrait.add(hop,forKey:"celebrate")}
    }
    private func say(_ text:String,opening:Bool){
        caption.string=text
        let opacity=CAKeyframeAnimation(keyPath:"opacity");opacity.values=[0,0.85,0.85,0];opacity.keyTimes=[0,0.16,0.72,1];opacity.duration=2.7;caption.add(opacity,forKey:"moment")
        if opening && motion{let appear=CABasicAnimation(keyPath:"opacity");appear.fromValue=0;appear.toValue=1;appear.duration=0.9;field.add(appear,forKey:"arrival");let enter=CABasicAnimation(keyPath:"opacity");enter.fromValue=0;enter.toValue=0.8;enter.duration=0.8;actor.add(enter,forKey:"arrival")}
    }
    override func mouseMoved(with event:NSEvent){
        guard motion,!paused,CACurrentMediaTime()-lastPointer>0.1 else{return};lastPointer=CACurrentMediaTime()
        let p=convert(event.locationInWindow,from:nil);guard p.y<field.bounds.height else{return}
        CATransaction.begin();CATransaction.setAnimationDuration(0.35)
        portrait.transform=CATransform3DMakeRotation(max(-0.10,min(0.10,(p.x-bounds.midX)/max(1,bounds.width)*0.2)),0,0,1)
        CATransaction.commit()
        CATransaction.begin();CATransaction.setDisableActions(true)
        hoverTile.frame=CGRect(x:floor(p.x/7)*7+0.5,y:floor(p.y/7)*7+0.5,width:5,height:5);hoverTile.backgroundColor=Self.color(style).withAlphaComponent(0.4).cgColor
        CATransaction.commit()
        let spark=CABasicAnimation(keyPath:"opacity");spark.fromValue=0.8;spark.toValue=0;spark.duration=0.8;hoverTile.add(spark,forKey:"pointer")
        let a=actor.presentation()?.position ?? actor.position
        if hypot(p.x-a.x,p.y-a.y)<48,CACurrentMediaTime()-lastGreeting>12{lastGreeting=CACurrentMediaTime();say("HEY",opening:false)}
    }
    override func mouseExited(with event:NSEvent){CATransaction.begin();CATransaction.setAnimationDuration(0.4);portrait.transform=CATransform3DIdentity;CATransaction.commit()}
    private func updatePlayback(){
        guard let root=layer else{return}
        let play = !isHidden && window?.isVisible == true && window?.isKeyWindow == true && motion
        if !play && !paused{let time=root.convertTime(CACurrentMediaTime(),from:nil);root.speed=0;root.timeOffset=time;paused=true}
        else if play && paused{let time=root.timeOffset;root.speed=1;root.timeOffset=0;root.beginTime=0;root.beginTime=root.convertTime(CACurrentMediaTime(),from:nil)-time;paused=false}
    }
    func suspend(){isHidden=true;updatePlayback()}
    var diagnosticPlayback:String{"hidden=\(isHidden) motion=\(motion) paused=\(paused) window=\(window?.windowNumber ?? -1) key=\(window?.isKeyWindow ?? false) visible=\(window?.isVisible ?? false) occluded=\(window?.occlusionState.contains(.visible) ?? false)"}
    var diagnosticAnimationCount:Int{actor.animationKeys()?.count ?? 0}
}
