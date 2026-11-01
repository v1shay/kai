import AppKit
@main struct NinjaAtmosphereHarness {
    @MainActor static func main(){
        _=NSApplication.shared
        if CommandLine.arguments.contains("--live"){NSApp.setActivationPolicy(.regular)}
        let scene=NinjaAtmosphereView(frame:CGRect(x:0,y:0,width:1000,height:500))
        scene.configure(identity:"preview",style:"fire",working:false,motion:false,visible:true)
        scene.layoutSubtreeIfNeeded()
        precondition(scene.diagnosticAnimationCount==0,"Reduced motion must be static")
        let colors=["fire","lightning","earth","ice","energy","water"]
        for style in colors {
            scene.configure(identity:style,style:style,working:true,motion:false,visible:true)
            let c=CGContext(data:nil,width:2000,height:1000,bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
            c.setFillColor(NSColor(srgbRed:0.065,green:0.072,blue:0.08,alpha:1).cgColor);c.fill(CGRect(x:0,y:0,width:2000,height:1000));c.translateBy(x:0,y:1000);c.scaleBy(x:2,y:-2);scene.layer!.render(in:c)
            let rep=NSBitmapImageRep(cgImage:c.makeImage()!)
            try! rep.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:"/private/tmp/kai-atmosphere-\(style).png"))
        }
        scene.configure(identity:"motion",style:"fire",working:true,motion:true,visible:true)
        precondition(scene.diagnosticAnimationCount>0,"Working scene must animate")
        scene.configure(identity:"motion",style:"fire",working:false,motion:false,visible:true)
        precondition(scene.diagnosticAnimationCount==0)
        if CommandLine.arguments.contains("--live"){
          DispatchQueue.main.async {
            let window=NSWindow(contentRect:CGRect(x:180,y:180,width:1000,height:500),styleMask:[.titled,.closable],backing:.buffered,defer:false)
            window.title="Kai Atmosphere Validation";window.backgroundColor=NSColor(srgbRed:0.065,green:0.072,blue:0.08,alpha:1);window.isReleasedWhenClosed=false;window.contentView=scene;window.makeKeyAndOrderFront(nil);window.orderFrontRegardless();NSApp.activate(ignoringOtherApps:true)
            let deadline=Date().addingTimeInterval(45)
            while !window.isKeyWindow && Date()<deadline {
                if let event=NSApp.nextEvent(matching:.any,until:Date().addingTimeInterval(0.05),inMode:.default,dequeue:true){NSApp.sendEvent(event)}
                NSApp.updateWindows()
            }
            if !window.isKeyWindow{window.close();print("NINJA_ATMOSPHERE_LIVE_SKIPPED_INACTIVE; static renders and inactive pause verified");NSApp.terminate(nil);return}
            RunLoop.current.run(until:Date().addingTimeInterval(0.4))
            scene.configure(identity:"live",style:"energy",working:true,motion:true,visible:true);window.displayIfNeeded();CATransaction.flush()
            RunLoop.current.run(until:Date().addingTimeInterval(0.35))
            let actor=scene.layer!.sublayers!.last!;let first=actor.presentation()!.position
            RunLoop.current.run(until:Date().addingTimeInterval(0.8));let second=actor.presentation()!.position
            fputs("LIVE_STATE active=\(NSApp.isActive) visible=\(window.isVisible) key=\(window.isKeyWindow) occlusion=\(window.occlusionState.rawValue) speed=\(scene.layer!.speed) first=\(first) second=\(second)\n",stderr)
            fputs("COMPONENT \(scene.diagnosticPlayback)\n",stderr)
            precondition(hypot(first.x-second.x,first.y-second.y)>1,"Native actor must really move")
            precondition(scene.hitTest(CGPoint(x:300,y:40))==nil,"Scene must never block chat input")
            let location=scene.convert(CGPoint(x:700,y:40),to:nil)
            let event=NSEvent.mouseEvent(with:.mouseMoved,location:location,modifierFlags:[],timestamp:ProcessInfo.processInfo.systemUptime,windowNumber:window.windowNumber,context:nil,eventNumber:1,clickCount:0,pressure:0)!
            scene.mouseMoved(with:event)
            precondition(scene.layer!.sublayers!.first!.sublayers![1].animation(forKey:"pointer") != nil,"Pointer should light a tile")
            window.orderOut(nil);RunLoop.current.run(until:Date().addingTimeInterval(0.2));precondition(scene.layer!.speed==0,"Hidden window must pause native animation")
            window.close();print("NINJA_ATMOSPHERE_LIVE_OK actual presentation movement, pointer tile, click-through, hidden pause");NSApp.terminate(nil)
          };NSApp.run()
        }
        scene.suspend();precondition(scene.isHidden)
        print("NINJA_ATMOSPHERE_OK six identity renders, static/accessibility, work and suspend")
    }
}
