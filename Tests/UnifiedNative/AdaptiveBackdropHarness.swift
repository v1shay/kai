import AppKit

/// A stand-alone bridge spy: no Code app, provider or chat data is loaded.
@MainActor final class EmbeddedCodeRuntime {
    static let shared=EmbeddedCodeRuntime()
    var packets=[[String:Any]]()
    func sendAction(_ action:String,_ fields:[String:Any]=[:]){precondition(action=="adaptive_contrast_samples");packets.append(fields);if packets.count>32{packets.removeFirst()}}
}
@main struct AdaptiveBackdropAudit {
    @MainActor static func main() {
        setbuf(stdout,nil);_=NSApplication.shared;NSApp.setActivationPolicy(.accessory)
        let window=NSWindow(contentRect:CGRect(x:200,y:180,width:600,height:400),styleMask:[.borderless],backing:.buffered,defer:false);window.isReleasedWhenClosed=false
        let host=NSView(frame:window.contentView!.bounds);host.wantsLayer=true;host.layer!.backgroundColor=NSColor(srgbRed:0.9,green:0,blue:0.9,alpha:1).cgColor;window.contentView=host
        window.level = .floating
        let sampler=AdaptiveBackdrop.shared
        func pump(_ seconds:Double){let end=Date().addingTimeInterval(seconds);while Date()<end{RunLoop.main.run(until:Date().addingTimeInterval(0.01))}}
        func count(_ label:String)->Int{Mirror(reflecting:Mirror(reflecting:sampler).children.first{$0.label==label}!.value).children.count}
        sampler.receive(["type":"adaptiveContrastConfigure","enabled":false]);pump(0.1)
        precondition(count("feeds")==0 && count("frames")==0)
        window.orderFrontRegardless();pump(0.2)
        sampler.registerNotch(window:window,host:host,area:host.bounds,active:true,frozen:false){_,rgb in rgb}
        sampler.receive(["type":"adaptiveContrastConfigure","enabled":true,"target":4.5,"settleMs":300])
        sampler.receive(["type":"adaptiveContrastRegion","id":"probe-chat","role":"chat","window":window.windowNumber,"rect":[0.0,0.0,600.0,400.0],"generation":100])
        sampler.receive(["type":"adaptiveContrastVisible","window":window.windowNumber,"ids":["probe-chat"]])
        pump(3)
        let packets=EmbeddedCodeRuntime.shared.packets
        let profiles=packets.flatMap{$0["profiles"] as? [[String:Any]] ?? []}
        let statuses=packets.compactMap{$0["status"] as? String}
        if !profiles.isEmpty{
            for profile in profiles{let pixels=profile["pixels"] as! [[Double]];precondition(pixels.count==64 && pixels.allSatisfy{$0.count==3 && $0.allSatisfy{$0.isFinite && $0>=0 && $0<=1}})}
            precondition(profiles.contains{$0["id"] as? String=="probe-chat"})
            sampler.receive(["type":"adaptiveContrastFreeze","window":window.windowNumber,"frozen":true]);pump(0.6)
            let frozenProfiles=EmbeddedCodeRuntime.shared.packets.flatMap{$0["profiles"] as? [[String:Any]] ?? []}.filter{$0["id"] as? String=="probe-chat"}
            precondition(frozenProfiles.last?["moving"] as? Bool == true,"Morph protection was not forwarded")
            sampler.receive(["type":"adaptiveContrastFreeze","window":window.windowNumber,"frozen":false]);pump(1.5)
            let settled=EmbeddedCodeRuntime.shared.packets.flatMap{$0["profiles"] as? [[String:Any]] ?? []}.filter{$0["id"] as? String=="probe-chat"}
            if settled.last?["moving"] as? Bool != false{for item in Mirror(reflecting:sampler).children where ["transitioning","frozen","lastPublish"].contains(item.label ?? ""){print("sampler state",item.label!,item.value)};print("window",window.isVisible,window.isOnActiveSpace,window.occlusionState.rawValue);print("settled profiles",settled.map{[$0["id"] as Any,$0["moving"] as Any]})};precondition(settled.last?["moving"] as? Bool == false,"Morph protection did not settle")
            let before=EmbeddedCodeRuntime.shared.packets.count
            for revision in 101...120{sampler.receive(["type":"adaptiveContrastRegion","id":"probe-chat","role":"chat","window":window.windowNumber,"rect":[0.0,0.0,Double(400+revision),400.0],"generation":revision])}
            pump(0.7);precondition(EmbeddedCodeRuntime.shared.packets.count-before<=4,"Geometry events bypassed sample throttling")
            let beforeTarget=EmbeddedCodeRuntime.shared.packets.count
            sampler.receive(["type":"adaptiveContrastConfigure","enabled":true,"target":7.0,"settleMs":900]);pump(0.6)
            precondition(EmbeddedCodeRuntime.shared.packets.count>beforeTarget,"Settings change did not re-publish cached backgrounds")
            print("PASS: live SCK feed -> panel samples -> native bridge, \(profiles.count) numeric panel profiles; no images saved")
        }else{
            precondition(statuses.contains("permission_required") || statuses.contains("capture_error"),"Sampler neither captured nor reported access state")
            print("PASS: capture access state without automatic permission request; live sampling unavailable in this validation process: \(statuses)")
        }
        sampler.receive(["type":"adaptiveContrastVisible","window":window.windowNumber,"ids":[String]()])
        sampler.registerNotch(window:window,host:host,area:host.bounds,active:false,frozen:false){_,rgb in rgb};window.orderOut(nil);pump(0.5)
        precondition(count("feeds")==0,"Hidden panels retained capture streams")
        sampler.receive(["type":"adaptiveContrastConfigure","enabled":false]);pump(0.2)
        precondition(count("feeds")==0 && count("frames")==0,"Disabled experiment retained screen frames")
        let after=EmbeddedCodeRuntime.shared.packets.count;pump(1);precondition(EmbeddedCodeRuntime.shared.packets.count==after,"Disabled sampler still publishes")
        print("PASS: hidden/disabled sampler stops streams, clears frames and has no recurring updates")
    }
}
