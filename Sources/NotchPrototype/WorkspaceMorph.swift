import AppKit

enum KaiMotion {
    // With linear x control points this is the same cubic ease-out as GPUI.
    static let interactionCurve = CAMediaTimingFunction(controlPoints: 1.0 / 3, 1, 2.0 / 3, 1)
    static let paneDuration: TimeInterval = 0.22
    static let controlDuration: TimeInterval = 0.16
    static let contentDuration: TimeInterval = 0.32
    static let cycleCurve = CAMediaTimingFunction(name: .easeInEaseOut)
    static let frameCurve = CAMediaTimingFunction(name: .linear)

    static func configure(_ animation: CAAnimation, inherited: Bool = false) {
        if let frames = animation as? CAKeyframeAnimation, frames.calculationMode == .discrete {
            animation.timingFunction = frameCurve
        } else if inherited {
            animation.timingFunction = frameCurve
        } else {
            animation.timingFunction = animation.repeatCount > 0 || animation.autoreverses
                ? cycleCurve : interactionCurve
        }
        if let group = animation as? CAAnimationGroup {
            group.animations?.forEach { configure($0, inherited: true) }
        }
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            animation.duration = 0.001
            animation.beginTime = 0
            animation.repeatCount = 0
            animation.autoreverses = false
        }
    }

    static func dismiss(_ window: NSWindow) {
        window.ignoresMouseEvents = true
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0 : controlDuration
            context.timingFunction = interactionCurve
            window.animator().alphaValue = 0
        }, completionHandler: {
            window.orderOut(nil)
        })
    }
}

extension CALayer {
    func addKai(_ animation: CAAnimation, forKey key: String?) {
        guard let configured = animation.copy() as? CAAnimation else { return }
        if NSWorkspace.shared.accessibilityDisplayShouldReduceMotion && animation.repeatCount > 0 {
            if let key { removeAnimation(forKey: key) }
            return
        }
        KaiMotion.configure(configured)
        add(configured, forKey: key)
    }
}

/// The actual workspace is clipped by one continuous native shape. GPUI paints
/// its real components at their final size; no tiles or per-frame window resize.
@MainActor final class WorkspaceMorph {
    var motionEnabled=true
    var openingOnlyOnce=false
    private var hasOpened=false
    private static let tracePerformance = ProcessInfo.processInfo.environment["KAI_PERFORMANCE_LOG"] != nil
    var onPresentation:((String)->Void)?
    private var revision=0
    private weak var window:NSWindow?,host:NSView?
    private var shell:NSPanel?,clip:CAShapeLayer?,previousMask:CALayer?
    private var glassBridge:WorkspaceGlassBridge?,opaqueFill:CALayer?
    private var contentOpacities:[(layer:CALayer,opacity:Float)]=[]
    private var delayed:DispatchWorkItem?,handoff:DispatchWorkItem?
    private weak var incomingNotch:NSPanel?
    private var incomingLevel:NSWindow.Level?
    private(set) var presentingNotch=false
    private var originalBackground:NSColor?,originalOpaque=false,originalShadow=true,originalIgnores=false
    private var originalLevel:NSWindow.Level?
    private(set) var expanded=false
    private(set) var transitioning=false
    private var surfaceRadius:CGFloat=10
    static let expansionDuration:TimeInterval=0.84
    static let collapseDuration:TimeInterval=0.60
    nonisolated static func destination(saved:CGRect?,screen:CGRect)->CGRect {screen}
    private let curve=KaiMotion.interactionCurve
    private func present(_ window:NSWindow){
        // GPUI's inactive-window key callback resigns synchronously while its
        // window-state lock is held. AppKit can reenter that same callback and
        // deadlock. Show the contour immediately; only assign key focus after
        // this application is actually active.
        if NSApp.isActive{window.makeKeyAndOrderFront(nil)}else{window.orderFront(nil)}
    }

    /// The incoming surface owns glass during an app-to-app handoff. Retiring
    /// the outgoing backdrop must not cancel its live contour or content fade.
    func retireBackdrop(){glassBridge?.hide()}

    private func clearVisuals(){
        delayed?.cancel();delayed=nil;handoff?.cancel();handoff=nil;presentingNotch=false
        if let incomingNotch,let incomingLevel{incomingNotch.level=incomingLevel};incomingNotch=nil;incomingLevel=nil
        retireBackdrop()
        for item in contentOpacities{
            item.layer.removeAnimation(forKey:"workspaceContent")
            CATransaction.begin();CATransaction.setDisableActions(true);item.layer.opacity=item.opacity;CATransaction.commit()
        };contentOpacities=[]
        opaqueFill?.removeFromSuperlayer()
        if let host,let clip,host.layer?.mask === clip{host.layer?.mask=previousMask}
        clip=nil;previousMask=nil;host=nil
        if let window,let originalBackground{
            window.backgroundColor=originalBackground;window.isOpaque=originalOpaque
            window.hasShadow=originalShadow;window.ignoresMouseEvents=originalIgnores
        }
        if let window,let originalLevel{window.level=originalLevel};originalLevel=nil
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
        animation.fillMode = .both;mask.addKai(animation,forKey:"workspaceShape")
    }
    nonisolated static func transitionLevel(_ level:NSWindow.Level)->NSWindow.Level{
        NSWindow.Level(rawValue:max(level.rawValue,NSWindow.Level.statusBar.rawValue+1))
    }
    private func prepare(_ window:NSWindow,from source:CGRect,to target:CGRect,duration:TimeInterval,delay:TimeInterval=0,expanding:Bool,contour:CGPath?=nil,sourceNotch:Bool?=nil,targetNotch:Bool?=nil,sourceRadius:CGFloat=14,targetRadius:CGFloat=14,backdrop:Bool=true){
        self.window=window
        originalLevel=window.level;window.level=Self.transitionLevel(window.level)
        originalBackground=window.backgroundColor;originalOpaque=window.isOpaque;originalShadow=window.hasShadow;originalIgnores=window.ignoresMouseEvents
        window.backgroundColor = .clear;window.isOpaque=false;window.hasShadow=false;window.ignoresMouseEvents=true
        guard let host=window.contentView?.superview ?? window.contentView else{return}
        self.host=host;host.wantsLayer=true
        let mask=CAShapeLayer();mask.frame=host.bounds;previousMask=host.layer?.mask;host.layer?.mask=mask;clip=mask
        animate(mask,from:contour.map{local($0,in:host,window:window)} ?? Self.outline(local(source,in:host,window:window),notch:sourceNotch ?? expanding,radius:sourceRadius),to:Self.outline(local(target,in:host,window:window),notch:targetNotch ?? !expanding,radius:targetRadius),duration:duration,delay:delay)
        // Fade foreground content separately, keeping the actual material and
        // tint alive beneath the same contour for the entire transition.
        if !WorkspaceGlassBridge.hasNativeGlass(window){
            let fill=opaqueFill ?? CALayer();opaqueFill=fill
            fill.frame=host.bounds;fill.backgroundColor=originalBackground?.cgColor
            host.layer?.insertSublayer(fill,at:0)
        }
        guard backdrop,let outside=WorkspaceGlassBridge.outsideBounds(source.union(target),excluding:window.frame) else{present(window);return}
        let bridge=glassBridge ?? WorkspaceGlassBridge();glassBridge=bridge;shell=bridge.panel
        bridge.configure(window:window,frame:outside,opaqueColor:originalBackground)
        animate(bridge.shape,from:contour.map{Self.convert($0){CGPoint(x:$0.x-outside.minX,y:$0.y-outside.minY)}} ?? Self.outline(source.offsetBy(dx:-outside.minX,dy:-outside.minY),notch:sourceNotch ?? expanding,radius:sourceRadius),to:Self.outline(target.offsetBy(dx:-outside.minX,dy:-outside.minY),notch:targetNotch ?? !expanding,radius:targetRadius),duration:duration,delay:delay)
        present(window);bridge.panel.order(.below,relativeTo:window.windowNumber)
    }
    private func liveContentOpacities()->[ObjectIdentifier:Float]{
        var values=Dictionary(uniqueKeysWithValues:contentOpacities.map{(ObjectIdentifier($0.layer),$0.layer.presentation()?.opacity ?? $0.layer.opacity)})
        // Translate progress when an interrupted app-to-app handoff switches
        // between fading the full source and fading just foreground content.
        if let hostLayer=host?.layer,let item=contentOpacities.first(where:{$0.opacity>0}){
            let progress=(item.layer.presentation()?.opacity ?? item.layer.opacity)/item.opacity
            let original=contentOpacities.first(where:{$0.layer === hostLayer})?.opacity ?? hostLayer.opacity
            values[ObjectIdentifier(hostLayer)]=original*progress
        }
        return values
    }
    private func animateContent(_ window:NSWindow,from values:[ObjectIdentifier:Float],appearing:Bool,duration:Double,includeGlass:Bool=false){
        let layers=includeGlass ? host?.layer.map{[$0]} ?? []:WorkspaceGlassBridge.foregroundLayers(window)
        for layer in layers{
            let original=layer.opacity
            contentOpacities.append((layer,original))
            let fade=CABasicAnimation(keyPath:"opacity")
            let inherited=host?.layer.flatMap{hostLayer in values[ObjectIdentifier(hostLayer)].map{original*$0/max(hostLayer.opacity,0.0001)}}
            let current:Float=layer.presentation()?.opacity ?? original
            let initial:Float=values[ObjectIdentifier(layer)] ?? inherited ?? (appearing ? 0:current)
            fade.fromValue=NSNumber(value:initial)
            fade.toValue=NSNumber(value:appearing ? original:0);fade.duration=duration;fade.timingFunction=curve
            layer.opacity=appearing ? original:0;layer.addKai(fade,forKey:"workspaceContent")
        }
    }
    func expand(_ window:NSWindow,from source:CGRect,replay:Bool=false,destination:CGRect?=nil,destinationLevel:NSWindow.Level?=nil,sourceNotch:Bool=true,crossfade:Bool=false,sourceRadius:CGFloat=10,destinationRadius:CGFloat=10,completion:@escaping()->Void={}){
        if !NSApp.isActive { NSApp.activate(ignoringOtherApps:true) }
        if expanded && !transitioning && !replay{present(window);completion();return}
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        let contour=transitioning ? liveContour():nil
        let returningOpacity=liveContentOpacities()
        let start=contour?.boundingBoxOfPath ?? source
        revision += 1;let token=revision;clearVisuals();self.window=window;expanded=true;transitioning=true
        // Restore the interrupted transition before applying the new form's
        // level; otherwise its old completion policy wins a rapid reversal.
        if let destinationLevel{window.level=destinationLevel}
        let screen=NSScreen.screens.first{$0.frame.contains(CGPoint(x:source.midX,y:source.maxY-1))} ?? window.screen ?? NSScreen.main
        let target=destination ?? Self.destination(saved:nil,screen:screen?.visibleFrame ?? window.frame)
        // A parked workspace retains its working-area frame. Reapplying that
        // identical frame forces AppKit/GPUI through a synchronous resize on
        // every reveal, although only the contour needs to move.
        surfaceRadius=destinationRadius
        let animateOpening = !openingOnlyOnce || !hasOpened
        if window.frame != target { window.setFrame(target, display: animateOpening) }
        hasOpened=true
        if !animateOpening {
            transitioning=false
            window.alphaValue=1
            onPresentation?("visible")
            let at = Self.tracePerformance ? CACurrentMediaTime() : 0
            present(window)
            if at != 0 { fputs("KAI_METRIC warm_order_front \((CACurrentMediaTime()-at)*1000)\n", stderr) }
            completion()
            return
        }
        onPresentation?(replay ? "reopen":"expand")
        let reduce = !motionEnabled || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        window.alphaValue=1
        if !reduce{
            prepare(window,from:start,to:target,duration:0.68,expanding:true,contour:contour,sourceNotch:sourceNotch,targetNotch:false,sourceRadius:sourceNotch ? 14:sourceRadius,targetRadius:destinationRadius)
            if !returningOpacity.isEmpty || crossfade{
                animateContent(window,from:returningOpacity,appearing:true,duration:crossfade ? 0.24:0.14)
            }
        }else{
            window.alphaValue=0;window.orderFront(nil)
            NSAnimationContext.runAnimationGroup{context in context.timingFunction=KaiMotion.interactionCurve; context.duration=0.12;window.animator().alphaValue=1}
        }
        let finish=DispatchWorkItem{[weak self,weak window] in
            guard let self,self.revision==token,let window else{return}
            self.transitioning=false;self.clearVisuals();window.alphaValue=1
            // The user may have switched applications while the reveal was in
            // flight. Finishing its mask must not order Code over that app.
            if NSApp.isActive{window.makeKeyAndOrderFront(nil)}
            completion()
        };delayed=finish;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.12:Self.expansionDuration),execute:finish)
    }
    func collapse(_ window:NSWindow,to target:CGRect,into notch:NSPanel?=nil,revealNotch:(()->Void)?=nil,targetNotch:Bool=true,retireContent:Bool=false,targetRadius:CGFloat=10,backdrop:Bool=true,completion:@escaping()->Void){
        if !expanded && !transitioning && !window.isVisible{completion();return}
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        let retiringOpacity=liveContentOpacities()
        let contour=transitioning ? liveContour():nil
        let source=contour?.boundingBoxOfPath ?? window.frame
        revision += 1;let token=revision;clearVisuals();self.window=window;expanded=false;transitioning=true
        onPresentation?("collapse")
        let reduce = !motionEnabled || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if !reduce{prepare(window,from:source,to:target,duration:0.48,delay:0.10,expanding:false,contour:contour,sourceNotch:false,targetNotch:targetNotch,sourceRadius:surfaceRadius,targetRadius:targetNotch ? 14:targetRadius,backdrop:backdrop)}else{
            NSAnimationContext.runAnimationGroup{context in context.timingFunction=KaiMotion.interactionCurve; context.duration=0.12;window.animator().alphaValue=0}
        }
        if !reduce && (notch != nil || !retiringOpacity.isEmpty || retireContent){
            // App-to-app transfers keep the original source retirement: the
            // incoming surface already owns the glass across that handoff.
            animateContent(window,from:retiringOpacity,appearing:false,duration:0.14,includeGlass:!backdrop)
        }
        if let notch,let revealNotch{
            // Keep only the continuous glass contour as Code retires. The real
            // notch grows concurrently above it; no workspace pixels enter it.
            incomingNotch=notch;incomingLevel=notch.level
            notch.level=NSWindow.Level(rawValue:window.level.rawValue+1)
            let reveal=DispatchWorkItem{[weak self] in
                guard let self,self.revision==token else{return}
                // A delayed render-server frame can leave the presentation
                // opacity behind the fade's deadline. Retire that animation
                // before revealing the native notch; workspace pixels must
                // never survive onto its surface.
                CATransaction.begin();CATransaction.setDisableActions(true)
                for item in self.contentOpacities{
                    item.layer.removeAnimation(forKey:"workspaceContent")
                    item.layer.opacity=0
                }
                CATransaction.commit();CATransaction.flush()
                self.presentingNotch=true;revealNotch()
            };handoff=reveal;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.06:0.16),execute:reveal)
        }
        let finish=DispatchWorkItem{[weak self,weak window] in
            guard let self,self.revision==token,let window else{return}
            window.orderOut(nil);self.transitioning=false;self.clearVisuals();window.alphaValue=1;self.onPresentation?("hidden");completion()
        };delayed=finish;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.12:Self.collapseDuration),execute:finish)
    }
    /// Restore the real workspace in place without ordering or activating it.
    /// A notch overlay must not close a window behind another application.
    func retainOpen(_ window:NSWindow){
        hasOpened=true
        if expanded && !transitioning{return}
        revision += 1;clearVisuals();self.window=window;expanded=true;transitioning=false;window.alphaValue=1;onPresentation?("visible")
    }
    func refocus(_ window:NSWindow) {
        retainOpen(window)
        present(window)
    }
    func park(_ window:NSWindow){revision += 1;clearVisuals();expanded=false;transitioning=false;window.orderOut(nil);window.alphaValue=1;onPresentation?("hidden")}
    func cancel(){revision += 1;transitioning=false;clearVisuals();window?.alphaValue=1}
}
