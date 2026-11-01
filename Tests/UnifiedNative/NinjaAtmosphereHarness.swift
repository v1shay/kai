import AppKit
@main struct NinjaAtmosphereHarness {
    @MainActor static func main(){
        _=NSApplication.shared
        if CommandLine.arguments.contains("--live"){NSApp.setActivationPolicy(.regular)}
        let suite="kai-ninja-phrases-test-"+UUID().uuidString,defaults=UserDefaults(suiteName:suite)!
        defer{defaults.removePersistentDomain(forName:suite)}
        var calendar=Calendar.current;calendar.timeZone=TimeZone.current
        let date=calendar.date(from:DateComponents(year:2026,month:11,day:2,hour:8))!
        var seen=Set<String>()
        for _ in 0..<45{let phrase=NinjaQuips.next(.arrival,name:"Zane",date:date,defaults:defaults);precondition(!phrase.contains("{name}"));precondition(phrase==phrase.lowercased(),"Ambient speech must be lowercase");precondition(seen.insert(phrase).inserted,"Arrival phrases must not repeat within their bag");}
        precondition(NinjaQuips.pool(.arrival,hour:8,weekday:2).contains("Top of the morning?"))
        precondition(!NinjaQuips.pool(.arrival,hour:23,weekday:2).contains("Top of the morning?"))
        precondition(NinjaQuips.pool(.arrival,hour:23,weekday:2).contains("The moon gets it."))
        for moment in [NinjaQuips.Moment.work,.done,.nearby]{var unique=Set<String>();for _ in 0..<15{let phrase=NinjaQuips.next(moment,name:"Kai",defaults:defaults);precondition(phrase==phrase.lowercased());precondition(unique.insert(phrase).inserted)}}
        precondition(NinjaAtmosphereView.actorOpacity(y:60,height:180)<0.3)
        precondition(NinjaAtmosphereView.actorOpacity(y:140,height:180)<NinjaAtmosphereView.actorOpacity(y:60,height:180))
        let actions:[NinjaPixelActor.Action]=[.rest,.walk,.run,.wave,.inspect,.leap,.bow,.climb,.tuck]
        let contact=CGContext(data:nil,width:8*96,height:actions.count*112,bitsPerComponent:8,bytesPerRow:0,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        contact.setFillColor(NSColor(srgbRed:0.065,green:0.072,blue:0.08,alpha:1).cgColor);contact.fill(CGRect(x:0,y:0,width:768,height:actions.count*112))
        for (row,action) in actions.enumerated(){var frames=Set<Data>();for i in 0..<8{let image=NinjaPixelActor.frame(style:"fire",action:action,step:i);frames.insert(image.dataProvider!.data! as Data);contact.draw(image,in:CGRect(x:i*96,y:(actions.count-1-row)*112,width:96,height:112))};precondition(frames.count>1,"Every action needs articulated movement")}
        let sheet=NSBitmapImageRep(cgImage:contact.makeImage()!);try! sheet.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:"/private/tmp/kai-ninja-actions.png"))
        for size in [CGSize(width:420,height:96),CGSize(width:1000,height:210)]{
            for working in [false,true]{
                let plan=NinjaMotionPlan(size:size,current:.zero,working:working,finished:false)
                precondition(plan.samples.contains{$0.action == .climb} && plan.samples.contains{$0.action == .tuck})
                let ys=plan.samples.map{$0.position.y};precondition(ys.max()!-ys.min()!>15,"Route must move vertically, not just animate limbs")
                precondition(plan.samples.allSatisfy{$0.position.y>=45 && $0.position.y<size.height && $0.position.x>=0 && $0.position.x<=size.width})
                precondition(hypot(plan.samples.first!.position.x-plan.samples.last!.position.x,plan.samples.first!.position.y-plan.samples.last!.position.y)<0.1,"Loop must close without a position jump")
                for (a,b) in zip(plan.samples,plan.samples.dropFirst()){precondition(hypot(a.position.x-b.position.x,a.position.y-b.position.y)<35,"Motion samples must remain continuous")}
            }
        }
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
            RunLoop.current.run(until:Date().addingTimeInterval(1.5))
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
        print("NINJA_ATMOSPHERE_OK six identity renders, static/accessibility, work and suspend; phrase bags, time-of-day, nine articulated actions, vertical routes, jumps and lowercase speech")
    }
}
