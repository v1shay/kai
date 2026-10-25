import AppKit

/// The actual workspace is clipped by one continuous native shape. GPUI paints
/// its real components at their final size; no tiles or per-frame window resize.
@MainActor final class WorkspaceMorph {
    var motionEnabled=true
    var onPresentation:((String)->Void)?
    private var revision=0
    private weak var window:NSWindow?,host:NSView?
    private var shell:NSPanel?,clip:CAShapeLayer?,previousMask:CALayer?
    private var delayed:DispatchWorkItem?
    private var originalBackground:NSColor?,originalOpaque=false,originalShadow=true,originalIgnores=false
    private(set) var expanded=false
    private(set) var transitioning=false
    static let expansionDuration:TimeInterval=0.84
    static let collapseDuration:TimeInterval=0.60
    nonisolated static func destination(saved:CGRect?,screen:CGRect)->CGRect {screen}
    private let curve=CAMediaTimingFunction(controlPoints:0.16,0.84,0.20,1)

    private func clearVisuals(){
        delayed?.cancel();delayed=nil;shell?.orderOut(nil);shell?.contentView=nil;shell=nil
        if let host,let clip,host.layer?.mask === clip{host.layer?.mask=previousMask}
        clip=nil;previousMask=nil;host=nil
        if let window,let originalBackground{
            window.backgroundColor=originalBackground;window.isOpaque=originalOpaque
            window.hasShadow=originalShadow;window.ignoresMouseEvents=originalIgnores
        }
        originalBackground=nil
    }
    private func liveContour()->CGPath?{
        guard let window,let host,let path=clip?.presentation()?.path ?? clip?.path else{return nil}
        return Self.convert(path){window.convertPoint(toScreen:host.convert($0,to:nil))}
    }
    nonisolated static func convert(_ path:CGPath,using map:(CGPoint)->CGPoint)->CGPath{
        let o=map(.zero),x=map(CGPoint(x:1,y:0)),y=map(CGPoint(x:0,y:1))
        var transform=CGAffineTransform(a:x.x-o.x,b:x.y-o.y,c:y.x-o.x,d:y.y-o.y,tx:o.x,ty:o.y)
        return path.copy(using:&transform)!
    }
    private func local(_ path:CGPath,in host:NSView,window:NSWindow)->CGPath{Self.convert(path){host.convert(window.convertPoint(fromScreen:$0),from:nil)}}
    private func local(_ rect:CGRect,in host:NSView,window:NSWindow)->CGRect{host.convert(window.convertFromScreen(rect),from:nil)}
    /// Same segment order and shoulder geometry as NotchSizing.path. Negative
    /// shoulders become ordinary app corners, so Core Animation interpolates
    /// one continuous contour rather than crossfading unrelated outlines.
    nonisolated static func outline(_ rect:CGRect,notch:Bool,radius:CGFloat)->CGPath{
        let x=rect.minX,right=rect.maxX,top=rect.maxY,bottom=rect.minY
        let r=min(max(0,radius),min(rect.width/2,rect.height/2)),sx:CGFloat=notch ? 7:-r,sy:CGFloat=notch ? 9:r,k:CGFloat=1-0.5523
        let p=CGMutablePath();p.move(to:CGPoint(x:x-sx,y:top));p.addLine(to:CGPoint(x:right+sx,y:top))
        p.addCurve(to:CGPoint(x:right,y:top-sy),control1:CGPoint(x:right+sx*k,y:top),control2:CGPoint(x:right,y:top-sy*k))
        p.addLine(to:CGPoint(x:right,y:bottom+r));p.addCurve(to:CGPoint(x:right-r,y:bottom),control1:CGPoint(x:right,y:bottom+r*k),control2:CGPoint(x:right-r*k,y:bottom))
        p.addLine(to:CGPoint(x:x+r,y:bottom));p.addCurve(to:CGPoint(x:x,y:bottom+r),control1:CGPoint(x:x+r*k,y:bottom),control2:CGPoint(x:x,y:bottom+r*k))
        p.addLine(to:CGPoint(x:x,y:top-sy));p.addCurve(to:CGPoint(x:x-sx,y:top),control1:CGPoint(x:x,y:top-sy*k),control2:CGPoint(x:x-sx*k,y:top));p.closeSubpath();return p
    }
    private func animate(_ mask:CAShapeLayer,from:CGPath,to:CGPath,duration:TimeInterval,delay:TimeInterval=0){
        CATransaction.begin();CATransaction.setDisableActions(true);mask.path=to;CATransaction.commit()
        let animation=CABasicAnimation(keyPath:"path");animation.fromValue=from;animation.toValue=to
        animation.duration=duration;animation.beginTime=CACurrentMediaTime()+delay;animation.timingFunction=curve
        animation.fillMode = .both;mask.add(animation,forKey:"workspaceShape")
    }
    private func prepare(_ window:NSWindow,from source:CGRect,to target:CGRect,duration:TimeInterval,delay:TimeInterval=0,expanding:Bool,contour:CGPath?=nil){
        self.window=window
        originalBackground=window.backgroundColor;originalOpaque=window.isOpaque;originalShadow=window.hasShadow;originalIgnores=window.ignoresMouseEvents
        window.backgroundColor = .clear;window.isOpaque=false;window.hasShadow=false;window.ignoresMouseEvents=true
        guard let host=window.contentView?.superview ?? window.contentView else{return}
        self.host=host;host.wantsLayer=true
        let mask=CAShapeLayer();mask.frame=host.bounds;previousMask=host.layer?.mask;host.layer?.mask=mask;clip=mask
        animate(mask,from:contour.map{local($0,in:host,window:window)} ?? Self.outline(local(source,in:host,window:window),notch:expanding,radius:expanding ? 14:10),to:Self.outline(local(target,in:host,window:window),notch:!expanding,radius:expanding ? 10:14),duration:duration,delay:delay)
        // A single glass backdrop bridges the menu-bar gap. It sits UNDER the
        // live workspace, so each real UI component is visible as it arrives.
        let union=source.union(target)
        let panel=NSPanel(contentRect:union,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=false;panel.ignoresMouseEvents=true;panel.level=window.level
        panel.collectionBehavior=[.fullScreenAuxiliary,.canJoinAllSpaces]
        let glass=NSVisualEffectView(frame:CGRect(origin:.zero,size:union.size));glass.wantsLayer=true;glass.material = .underWindowBackground;glass.blendingMode = .behindWindow;glass.state = .active
        panel.contentView=glass;shell=panel
        let glassMask=CAShapeLayer();glass.layer?.mask=glassMask
        animate(glassMask,from:contour.map{Self.convert($0){CGPoint(x:$0.x-union.minX,y:$0.y-union.minY)}} ?? Self.outline(source.offsetBy(dx:-union.minX,dy:-union.minY),notch:expanding,radius:expanding ? 14:10),to:Self.outline(target.offsetBy(dx:-union.minX,dy:-union.minY),notch:!expanding,radius:expanding ? 10:14),duration:duration,delay:delay)
        window.makeKeyAndOrderFront(nil);panel.order(.below,relativeTo:window.windowNumber)
    }
    func expand(_ window:NSWindow,from source:CGRect,completion:@escaping()->Void={}){
        if expanded && !transitioning{window.makeKeyAndOrderFront(nil);completion();return}
        let contour=transitioning ? liveContour():nil
        let start=contour?.boundingBoxOfPath ?? source
        revision += 1;let token=revision;clearVisuals();self.window=window;expanded=true;transitioning=true
        let screen=NSScreen.screens.first{$0.frame.contains(CGPoint(x:source.midX,y:source.maxY-1))} ?? window.screen ?? NSScreen.main
        let target=Self.destination(saved:nil,screen:screen?.visibleFrame ?? window.frame)
        window.setFrame(target,display:true);onPresentation?("expand")
        let reduce = !motionEnabled || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        window.alphaValue=1
        if !reduce{prepare(window,from:start,to:target,duration:0.68,expanding:true,contour:contour)}else{
            window.alphaValue=0;window.orderFront(nil)
            NSAnimationContext.runAnimationGroup{context in context.duration=0.12;window.animator().alphaValue=1}
        }
        NSApp.activate(ignoringOtherApps:true)
        let finish=DispatchWorkItem{[weak self,weak window] in
            guard let self,self.revision==token,let window else{return}
            self.transitioning=false;self.clearVisuals();window.alphaValue=1;window.makeKeyAndOrderFront(nil);completion()
        };delayed=finish;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.12:Self.expansionDuration),execute:finish)
    }
    func collapse(_ window:NSWindow,to target:CGRect,completion:@escaping()->Void){
        if !expanded && !transitioning && !window.isVisible{completion();return}
        let contour=transitioning ? liveContour():nil
        let source=contour?.boundingBoxOfPath ?? window.frame
        revision += 1;let token=revision;clearVisuals();self.window=window;expanded=false;transitioning=true
        onPresentation?("collapse")
        let reduce = !motionEnabled || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if !reduce{prepare(window,from:source,to:target,duration:0.48,delay:0.10,expanding:false,contour:contour)}else{
            NSAnimationContext.runAnimationGroup{context in context.duration=0.12;window.animator().alphaValue=0}
        }
        let finish=DispatchWorkItem{[weak self,weak window] in
            guard let self,self.revision==token,let window else{return}
            window.orderOut(nil);self.transitioning=false;self.clearVisuals();window.alphaValue=1;self.onPresentation?("hidden");completion()
        };delayed=finish;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.12:Self.collapseDuration),execute:finish)
    }
    func park(_ window:NSWindow){revision += 1;clearVisuals();expanded=false;transitioning=false;window.orderOut(nil);window.alphaValue=1;onPresentation?("hidden")}
    func cancel(){revision += 1;transitioning=false;clearVisuals();window?.alphaValue=1}
}
