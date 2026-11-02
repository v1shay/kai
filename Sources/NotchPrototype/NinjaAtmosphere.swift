import AppKit
import QuartzCore

/// A transparent, non-interactive scene. Motion lives on the render server;
/// no display link, polling, GPU readback, or conversation rerender timer.
@MainActor final class NinjaAtmosphereView:NSView {
    enum Phase:String {case idle,working,finished}
    private let field=CALayer(),grid=CALayer(),hoverTile=CALayer(),actionTile=CALayer(),actor=CALayer(),portrait=CALayer(),caption=CATextLayer()
    private var tiles=[CALayer](),observers=[NSObjectProtocol]()
    private var identity="",name="Ninja",style="fire",phase=Phase.idle,motion=true,paused=false
    private var rasterKey="",sceneKey="",lastPointer:CFTimeInterval=0
    private var sceneEpoch:CFTimeInterval?
    private var tracking:NSTrackingArea?
    private var lastGreeting:CFTimeInterval=0
    override var isFlipped:Bool{true}
    override func hitTest(_ point:NSPoint)->NSView?{nil}
    override init(frame:NSRect){
        super.init(frame:frame);wantsLayer=true;layer?.masksToBounds=true
        layer?.addSublayer(field);field.addSublayer(grid);field.addSublayer(hoverTile);hoverTile.opacity=0;hoverTile.cornerRadius=0.8;field.addSublayer(actionTile);actionTile.opacity=0;actionTile.cornerRadius=0.8;layer?.addSublayer(actor);actor.addSublayer(portrait);actor.addSublayer(caption)
        caption.font=NSFont.systemFont(ofSize:10,weight:.medium);caption.fontSize=10;caption.alignmentMode = .center;caption.isWrapped=true;caption.opacity=0
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
    func configure(identity:String,name:String="Ninja",style:String,working:Bool,motion:Bool,visible:Bool){
        let opening=self.identity != identity || (isHidden && visible)
        let completed=self.phase == .working && !working && !opening
        let starting=working && self.phase != .working
        let changed=self.style != style
        self.identity=identity;self.name=name;self.style=style;self.motion=motion && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        phase=working ? .working:(completed ? .finished:(opening ? .idle:phase == .finished ? .finished:.idle))
        isHidden = !visible
        if changed{rasterKey=""}
        if !visible{updatePlayback();return}
        if rasterKey.isEmpty{needsLayout=true};layoutSubtreeIfNeeded()
        animateScene()
        if visible && (opening || completed || starting) {say(NinjaQuips.next(completed ? .done:working ? .work:.arrival,name:name),opening:opening)}
        updatePlayback()
    }
    override func layout(){
        super.layout();guard !isHidden,bounds.width>0,bounds.height>0 else{return}
        CATransaction.begin();CATransaction.setDisableActions(true)
        let height=min(210,max(96,bounds.height*0.34));field.frame=CGRect(x:0,y:0,width:bounds.width,height:height)
        let scale=window?.backingScaleFactor ?? 2
        let key="\(Int(bounds.width*scale)):\(Int(height*scale)):\(style)"
        if key != rasterKey {rasterKey=key;grid.frame=field.bounds;grid.contentsScale=scale;grid.contents=makeGrid(size:field.bounds.size,scale:scale);sceneKey=""}
        let spriteSize=Self.spriteSize(fieldHeight:height)
        portrait.frame=CGRect(x:-spriteSize.width/2,y:-spriteSize.height/2,width:spriteSize.width,height:spriteSize.height);portrait.contentsGravity = .resizeAspect;portrait.magnificationFilter = .nearest
        portrait.contents=NinjaPixelActor.frame(style:style,action:.rest,step:0)
        let captionWidth=min(260,max(96,bounds.width*0.52-16))
        caption.frame=CGRect(x:-captionWidth/2,y:hypot(spriteSize.width,spriteSize.height)/2+4,width:captionWidth,height:28);caption.contentsScale=scale;caption.foregroundColor=Self.color(style).withAlphaComponent(0.8).cgColor
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
    private func animateScene(){
        guard bounds.width>0 else{return}
        let key="\(identity)/\(style)/\(phase.rawValue)/\(motion)/\(rasterKey)"
        guard key != sceneKey else{return};sceneKey=key
        let current=actor.presentation()?.position ?? actor.position
        actor.removeAllAnimations();portrait.removeAllAnimations();field.removeAllAnimations();actionTile.removeAllAnimations();tiles.forEach{$0.removeAllAnimations()}
        let working=phase == .working
        let plan=NinjaMotionPlan(size:field.bounds.size,working:working,finished:phase == .finished)
        CATransaction.begin();CATransaction.setDisableActions(true)
        actionTile.bounds=CGRect(x:0,y:0,width:6,height:6);actionTile.backgroundColor=Self.color(style).cgColor
        actor.position = !motion && current != .zero ? current:plan.samples[0].position;actor.opacity=Self.actorOpacity(y:actor.position.y,height:field.bounds.height);field.opacity=1
        CATransaction.commit()
        guard motion else{return}
        if sceneEpoch == nil{sceneEpoch=layer!.convertTime(CACurrentMediaTime(),from:nil)}
        let travel=CAKeyframeAnimation(keyPath:"position")
        travel.values=plan.samples.map{NSValue(point:$0.position)};travel.duration=plan.duration;travel.repeatCount = .infinity;travel.calculationMode = .linear;travel.beginTime=sceneEpoch!;actor.add(travel,forKey:"footwork")
        let gait=CAKeyframeAnimation(keyPath:"contents")
        gait.values=plan.samples.map{NinjaPixelActor.frame(style:style,action:$0.action,step:$0.step,left:$0.left)};gait.duration=plan.duration;gait.repeatCount = .infinity;gait.calculationMode = .discrete;gait.beginTime=sceneEpoch!;portrait.add(gait,forKey:"actions")
        let tumble=CAKeyframeAnimation(keyPath:"transform.rotation.z");tumble.values=plan.samples.map{$0.rotation};tumble.duration=plan.duration;tumble.repeatCount = .infinity;tumble.calculationMode = .discrete;tumble.beginTime=sceneEpoch!;portrait.add(tumble,forKey:"tumble")
        let depth=CAKeyframeAnimation(keyPath:"opacity");depth.values=plan.samples.map{Self.actorOpacity(y:$0.position.y,height:field.bounds.height)};depth.duration=plan.duration;depth.repeatCount = .infinity;depth.beginTime=sceneEpoch!;actor.add(depth,forKey:"depth")
        let target=CAKeyframeAnimation(keyPath:"position");target.values=plan.samples.map{NSValue(point:CGPoint(x:floor(($0.position.x+12)/7)*7+3.5,y:floor(($0.position.y+12)/7)*7+3.5))};target.duration=plan.duration;target.repeatCount = .infinity;target.calculationMode = .discrete;target.beginTime=sceneEpoch!;actionTile.add(target,forKey:"target")
        let tap=CAKeyframeAnimation(keyPath:"opacity");tap.values=plan.samples.map{sample -> Float in let contact=sample.action == .inspect || sample.action == .climb;return contact ? Self.actorOpacity(y:sample.position.y,height:field.bounds.height)*(sample.step%4<2 ? 0.8:0.3):0};tap.duration=plan.duration;tap.repeatCount = .infinity;tap.calculationMode = .discrete;tap.beginTime=sceneEpoch!;actionTile.add(tap,forKey:"glow")
        for (i,tile) in tiles.enumerated(){
            let wave=CAKeyframeAnimation(keyPath:"opacity");let base=tile.opacity
            wave.values=[base*0.35,base*(working ? 1.9:1.1),base*0.35];wave.duration=working ? 3.6:8.5;wave.beginTime=sceneEpoch!+Double(i%11)*0.19;wave.repeatCount = .infinity;wave.timingFunction=CAMediaTimingFunction(name:.easeInEaseOut);tile.add(wave,forKey:"glimmer")
            let drift=CABasicAnimation(keyPath:"transform.translation.y");drift.fromValue=2;drift.toValue = -3;drift.autoreverses=true;drift.duration=Double(4+i%5);drift.repeatCount = .infinity;drift.beginTime=sceneEpoch!;tile.add(drift,forKey:"drift")
        }
    }
    static func spriteSize(fieldHeight:CGFloat)->CGSize{let height=min(38,fieldHeight/4);return CGSize(width:height*32/38,height:height)}
    // The lower part of the field is only a feather, not a walking surface.
    static func dotTravelDepth(fieldHeight:CGFloat)->CGFloat{fieldHeight*0.60}
    static func actorOpacity(y:CGFloat,height:CGFloat)->Float{Float(0.42*pow(max(0.24,1-y/max(1,height)),1.15))}
    private func say(_ text:String,opening:Bool){
        caption.string=text
        guard motion else{caption.removeAllAnimations();return}
        let opacity=CAKeyframeAnimation(keyPath:"opacity");opacity.values=[0,0.95,0.95,0];opacity.keyTimes=[0,0.12,0.82,1];opacity.duration=5.2;caption.add(opacity,forKey:"moment")
        if opening && motion{let appear=CABasicAnimation(keyPath:"opacity");appear.fromValue=0;appear.toValue=1;appear.duration=0.9;field.add(appear,forKey:"arrival");let enter=CABasicAnimation(keyPath:"opacity");enter.fromValue=0;enter.toValue=actor.opacity;enter.duration=0.8;actor.add(enter,forKey:"arrival")}
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
        if hypot(p.x-a.x,p.y-a.y)<48,CACurrentMediaTime()-lastGreeting>30{lastGreeting=CACurrentMediaTime();say(NinjaQuips.next(.nearby,name:name),opening:false);let wave=CAKeyframeAnimation(keyPath:"contents");wave.values=(0..<16).map{NinjaPixelActor.frame(style:style,action:.wave,step:$0)};wave.duration=1.3;wave.calculationMode = .discrete;portrait.add(wave,forKey:"wave")}
    }
    override func mouseExited(with event:NSEvent){CATransaction.begin();CATransaction.setAnimationDuration(0.4);portrait.transform=CATransform3DIdentity;CATransaction.commit()}
    private func updatePlayback(){
        guard let root=layer else{return}
        let play = !isHidden && window?.isVisible == true && (window?.isKeyWindow == true || window is NSPanel) && motion
        if !play && !paused{let time=root.convertTime(CACurrentMediaTime(),from:nil);root.speed=0;root.timeOffset=time;paused=true}
        else if play && paused{let time=root.timeOffset;root.speed=1;root.timeOffset=0;root.beginTime=0;root.beginTime=root.convertTime(CACurrentMediaTime(),from:nil)-time;paused=false}
    }
    func mount(in host:NSView,frame:NSRect){
        CATransaction.begin();CATransaction.setDisableActions(true)
        if superview !== host{removeFromSuperview();host.addSubview(self,positioned:.above,relativeTo:nil)}
        if self.frame != frame{self.frame=frame}
        CATransaction.commit()
    }
    func suspend(){isHidden=true;updatePlayback()}
    var diagnosticPosition:CGPoint?{actor.presentation()?.position}
    var diagnosticEpoch:CFTimeInterval?{sceneEpoch}
    var diagnosticPlayback:String{"hidden=\(isHidden) motion=\(motion) paused=\(paused) window=\(window?.windowNumber ?? -1) key=\(window?.isKeyWindow ?? false) visible=\(window?.isVisible ?? false) occluded=\(window?.occlusionState.contains(.visible) ?? false)"}
    var diagnosticAnimationCount:Int{actor.animationKeys()?.count ?? 0}
}

/// Sample a short action score once, then let the render server play it. Each
/// jump has a ballistic arc and landing; climbs animate alternating hands/feet.
@MainActor struct NinjaMotionPlan {
    struct Sample {let position:CGPoint;let action:NinjaPixelActor.Action;let step:Int;let left:Bool;let rotation:Double}
    let samples:[Sample],duration:Double
    init(size:CGSize,working:Bool,finished:Bool){
        let padding=min(146,max(48,size.width*0.26)),targetLeft=padding,targetRight=max(targetLeft,size.width-padding)
        let sprite=NinjaAtmosphereView.spriteSize(fieldHeight:size.height)
        let radius=hypot(sprite.width,sprite.height)/2
        // Reserve the whole rotated sprite, including a landing/pose margin.
        let targetTop=radius+6,targetBottom=max(targetTop,NinjaAtmosphereView.dotTravelDepth(fieldHeight:size.height)-radius-2)
        // One design-space score: resizing changes its coordinates, never its
        // clock, leg boundaries, jump phase or which side the actor is on.
        let left:CGFloat=146,right:CGFloat=854,top:CGFloat=56,bottom:CGFloat=182
        let home=CGPoint(x:left,y:top)
        let span=right-left,far=home.x<(left+right)/2 ? right:left
        let low=CGPoint(x:left+span*0.38,y:bottom),ledge=CGPoint(x:far,y:bottom)
        let roof=CGPoint(x:far,y:top),across=CGPoint(x:far==right ? left:right,y:top+10)
        let middle=CGPoint(x:left+span*0.65,y:bottom-7)
        struct Leg {let from:CGPoint,to:CGPoint,action:NinjaPixelActor.Action,seconds:Double,jump:Bool,flip:Bool}
        var legs=[Leg](),cursor=home
        func add(_ to:CGPoint,_ action:NinjaPixelActor.Action,_ seconds:Double,jump:Bool=false,flip:Bool=false){legs.append(Leg(from:cursor,to:to,action:action,seconds:seconds,jump:jump,flip:flip));cursor=to}
        // Route timing is independent of surface size and working/idle state.
        // This keeps the same normalized position when the live view moves hosts.
        func stride(_ to:CGPoint)->Double{2.4}
        add(home,finished ? .bow:.wave,1.2)
        let floor=CGPoint(x:home.x,y:bottom)
        add(floor,.leap,0.85,jump:true);add(floor,.inspect,0.22)
        add(low,working ? .run:.walk,stride(low))
        add(ledge,.leap,0.95,jump:true);add(ledge,.inspect,0.28)
        add(ledge,.inspect,1.1)
        add(roof,.climb,2.4)
        add(roof,.rest,0.65)
        add(across,.tuck,1.3,jump:true,flip:true);add(across,.inspect,0.28)
        add(across,.wave,0.9)
        add(middle,.leap,1.2,jump:true);add(middle,.inspect,0.28)
        add(middle,.inspect,1.4)
        add(home,.tuck,1.2,jump:true,flip:true);add(home,.inspect,0.28)
        add(home,finished ? .bow:.rest,1.2)
        let total=legs.reduce(0){$0+$1.seconds},count=Int(ceil(total*24))
        var frames=[Sample](),legIndex=0,legStart=0.0
        for i in 0...count {
            let time=Double(i)/Double(count)*total
            while legIndex<legs.count-1 && time>legStart+legs[legIndex].seconds{legStart+=legs[legIndex].seconds;legIndex+=1}
            let leg=legs[legIndex],t=min(1,max(0,(time-legStart)/leg.seconds))
            let arc=leg.jump ? min(32,max(6,Double(min(leg.from.y,leg.to.y)-46)))*4*t*(1-t):0
            let grip=leg.action == .climb ? sin(t*Double.pi*10)*2:0
            let point=CGPoint(x:leg.from.x+(leg.to.x-leg.from.x)*t+grip,y:leg.from.y+(leg.to.y-leg.from.y)*t-arc)
            let direction=leg.to.x<leg.from.x
            let step=leg.jump ? min(7,Int(t*8)):Int((time-legStart)*(working ? 16:10))%8
            // Ease into and out of a somersault; end on the exact upright pose.
            let spin=leg.flip ? (t*t*(3-2*t))*Double.pi*2*(direction ? -1:1):0
            let mapped=CGPoint(x:targetLeft+(point.x-left)/(right-left)*(targetRight-targetLeft),y:targetTop+(point.y-top)/(bottom-top)*(targetBottom-targetTop))
            frames.append(Sample(position:mapped,action:leg.action,step:step,left:direction,rotation:spin))
        }
        samples=frames;duration=total
    }
}
