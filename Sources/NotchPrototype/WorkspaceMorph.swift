import AppKit

/// A finite native glass/fragment transition between one notch and one workspace.
/// Code is laid out once at its final size; its renderer is never resized per frame.
@MainActor final class WorkspaceMorph {
    private var revision=0
    private var expandedFrame:CGRect?
    private weak var window:NSWindow?
    private var shell:NSPanel?,surface:NSVisualEffectView?
    private var delayed:DispatchWorkItem?
    private(set) var expanded=false
    private(set) var transitioning=false
    nonisolated static func destination(saved:CGRect?,screen:CGRect)->CGRect {
        let usable=screen.insetBy(dx:24,dy:24)
        let requested=saved ?? CGRect(x:usable.midX-600,y:usable.midY-390,width:1200,height:780)
        let width=min(max(940,requested.width),usable.width),height=min(max(620,requested.height),usable.height)
        return CGRect(x:min(max(requested.minX,usable.minX),usable.maxX-width),y:min(max(requested.minY,usable.minY),usable.maxY-height),width:width,height:height)
    }
    static func image(of view:NSView,in rect:CGRect)->CGImage?{
        guard rect.width>0,rect.height>0,let context=CGContext(data:nil,width:Int(ceil(rect.width*2)),height:Int(ceil(rect.height*2)),bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)else{return nil}
        context.scaleBy(x:2,y:2);context.translateBy(x:-rect.minX,y:-rect.minY)
        (view.layer?.presentation() ?? view.layer)?.render(in:context)
        return context.makeImage()
    }
    private func clearVisuals(){delayed?.cancel();delayed=nil;shell?.orderOut(nil);shell?.contentView=nil;shell=nil;surface=nil}
    private func veil(from source:CGRect,to target:CGRect,image:CGImage?,expanding:Bool){
        let union=source.union(target).insetBy(dx:-12,dy:-12)
        let panel=NSPanel(contentRect:union,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=false;panel.ignoresMouseEvents=true;panel.level = .popUpMenu;panel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary]
        let effect=NSVisualEffectView(frame:CGRect(origin:.zero,size:union.size));effect.wantsLayer=true;effect.material = .hudWindow;effect.blendingMode = .behindWindow;effect.state = .active
        effect.layer?.backgroundColor=NSColor(calibratedWhite:0.035,alpha:0.88).cgColor
        panel.contentView=effect;shell=panel;surface=effect
        let a=source.offsetBy(dx:-union.minX,dy:-union.minY),b=target.offsetBy(dx:-union.minX,dy:-union.minY)
        let mask=CAShapeLayer();effect.layer?.mask=mask
        let first=CGPath(roundedRect:a,cornerWidth:expanding ? 17:22,cornerHeight:expanding ? 17:22,transform:nil)
        let last=CGPath(roundedRect:b,cornerWidth:expanding ? 22:17,cornerHeight:expanding ? 22:17,transform:nil)
        mask.path=last
        let duration=0.62,curve=CAMediaTimingFunction(controlPoints:0.16,0.84,0.22,1)
        let shape=CABasicAnimation(keyPath:"path");shape.fromValue=first;shape.toValue=last;shape.duration=duration;shape.timingFunction=curve;mask.add(shape,forKey:"surfaceMorph")
        let rim=CAShapeLayer();rim.path=last;rim.fillColor=nil;rim.strokeColor=NSColor.white.withAlphaComponent(0.18).cgColor;rim.lineWidth=0.8;effect.layer?.addSublayer(rim);rim.add(shape,forKey:"rimMorph")
        let sheen=CAGradientLayer();sheen.frame=CGRect(origin:.zero,size:union.size);sheen.colors=[NSColor.clear.cgColor,NSColor.white.withAlphaComponent(0.1).cgColor,NSColor.clear.cgColor];sheen.locations=[0,0.48,1];sheen.startPoint=CGPoint(x:0,y:0);sheen.endPoint=CGPoint(x:1,y:1);effect.layer?.addSublayer(sheen)
        let sweep=CABasicAnimation(keyPath:"transform.translation.x");sweep.fromValue = -union.width*0.5;sweep.toValue=union.width*0.6;sweep.duration=duration;sheen.add(sweep,forKey:"lightSweep")
        if let image,expanding {
            // One shared texture, 32 bounded fragments, no timers or readbacks.
            let cols=8,rows=4
            for y in 0..<rows{for x in 0..<cols{
                let tile=CALayer(),u=CGFloat(x)/CGFloat(cols),v=CGFloat(y)/CGFloat(rows)
                tile.contents=image;tile.contentsRect=CGRect(x:u,y:v,width:1/CGFloat(cols),height:1/CGFloat(rows))
                tile.frame=CGRect(x:a.minX+u*a.width,y:a.minY+v*a.height,width:a.width/CGFloat(cols)+0.5,height:a.height/CGFloat(rows)+0.5)
                effect.layer?.addSublayer(tile)
                let position=CABasicAnimation(keyPath:"position");position.fromValue=tile.position;position.toValue=CGPoint(x:b.minX+(u+0.0625)*b.width,y:b.minY+(v+0.125)*b.height);position.duration=duration;position.timingFunction=curve;position.isRemovedOnCompletion=false;position.fillMode = .forwards;tile.add(position,forKey:"fragmentTravel")
                let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=1;fade.toValue=0;fade.beginTime=CACurrentMediaTime()+0.06+Double(x+y)*0.012;fade.duration=0.3;fade.fillMode = .forwards;fade.isRemovedOnCompletion=false;tile.add(fade,forKey:"fragmentDissolve")
            }}
        }
        panel.orderFrontRegardless()
    }
    func expand(_ window:NSWindow,from source:CGRect,image:CGImage?=nil,completion:@escaping()->Void={}) {
        if expanded && !transitioning{window.makeKeyAndOrderFront(nil);completion();return}
        revision += 1;let token=revision;clearVisuals();expanded=true;transitioning=true;self.window=window
        let screen=window.screen ?? NSScreen.main
        let target=Self.destination(saved:expandedFrame ?? window.frame,screen:screen?.visibleFrame ?? window.frame);expandedFrame=target
        window.setFrame(target,display:true);window.alphaValue=0;window.orderFront(nil);NSApp.activate(ignoringOtherApps:true)
        let reduce=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if !reduce{veil(from:source,to:target,image:image,expanding:true)}
        let work=DispatchWorkItem{[weak self,weak window] in
            guard let self,self.revision==token,let window else{return}
            NSAnimationContext.runAnimationGroup{context in context.duration=reduce ? 0.1:0.29;window.animator().alphaValue=1}
            // AppKit may omit animator completions for interrupted or inactive
            // windows. A finite owner timer always settles the presentation.
            let finish=DispatchWorkItem{[weak self,weak window] in
                guard let self,self.revision==token,let window else{return}
                window.alphaValue=1;self.transitioning=false;self.clearVisuals();window.makeKeyAndOrderFront(nil);completion()
            };self.delayed=finish;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.1:0.30),execute:finish)
        };delayed=work;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0:0.32),execute:work)
    }
    func collapse(_ window:NSWindow,to target:CGRect,completion:@escaping()->Void) {
        if !expanded && !transitioning && !window.isVisible{completion();return}
        revision += 1;let token=revision
        if !transitioning{expandedFrame=window.frame}
        let source=window.frame;clearVisuals();expanded=false;transitioning=true;self.window=window
        let reduce=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if !reduce{veil(from:source,to:target,image:nil,expanding:false)}
        NSAnimationContext.runAnimationGroup{context in context.duration=reduce ? 0.1:0.24;window.animator().alphaValue=0}
        let work=DispatchWorkItem{[weak self,weak window] in
            guard let self,self.revision==token,let window else{return};window.orderOut(nil);window.alphaValue=1;self.transitioning=false;self.clearVisuals();completion()
        };delayed=work;DispatchQueue.main.asyncAfter(deadline:.now()+(reduce ? 0.1:0.62),execute:work)
    }
    func park(_ window:NSWindow){revision += 1;clearVisuals();if !transitioning{expandedFrame=window.frame};expanded=false;transitioning=false;window.orderOut(nil);window.alphaValue=1}
    func cancel(){revision += 1;transitioning=false;clearVisuals();window?.alphaValue=1}
}
