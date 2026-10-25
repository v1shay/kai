import AppKit

private func auditCommand(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{
    guard let bytes else{return false};let data=Data(bytes:bytes,count:count)
    MainActor.assumeIsolated{if let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any]{KaiStateAudit.commands.append(value)}};return true
}
private func unwrap(_ value:Any)->Any{
    let mirror=Mirror(reflecting:value)
    if mirror.displayStyle == .optional,let child=mirror.children.first{return unwrap(child.value)}
    return value
}
private func member(_ object:Any,_ name:String)->Any?{Mirror(reflecting:object).children.first{$0.label == name}.map{unwrap($0.value)}}
@main struct KaiStateAudit {
 @MainActor static var commands=[[String:Any]]()
 @MainActor static func main()throws{
    _=NSApplication.shared
    let defaults=UserDefaults.standard
    defaults.set(true,forKey:"unifiedPreferencesSeeded")
    defaults.set(true,forKey:"floatingTasks.enabled")
    defaults.set(false,forKey:"soundsEnabled")
    defaults.set(false,forKey:"speakResponses")
    defaults.set(false,forKey:"youtubeMedia");defaults.set(false,forKey:"spotifyMedia")
    let window=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
    window.isReleasedWhenClosed=false
    let runtime=EmbeddedCodeRuntime.shared;runtime.start(window:window,command:auditCommand)
    guard let app=member(runtime,"app") as? KaiPetApp,let pets=member(app,"floatingTasks") as? FloatingTaskPets else{fatalError("Audit reflection failed")}
    defer{
        app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification))
        NSApp.windows.forEach{$0.orderOut(nil)}
    }
    func packet(scene:String,run:String?,finished:Bool=false)->Data{
        let tasks:[[String:Any]]=run.map{[["id":"code:fixture","name":"Fixture","runId":$0,"scene":"thinking","text":"Working","agent":"codex"]]} ?? []
        var object:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:fixture","threadName":"Fixture","history":[],"readOnly":false,"approval":false,"activity":["scene":scene,"text":scene],"sessionActivity":["scene":scene,"text":scene],"activeTasks":tasks,"notice":"Audit-\(scene)-\(run ?? "none")","attachments":[],"selectedAgent":"codex","models":[],"ownerToken":"audit","composerDraft":""]
        object["activeTurnId"]=run ?? NSNull() as Any
        object["finishedTasks"]=finished ? [["id":"code:fixture","runId":"3"]]:[]
        return try! JSONSerialization.data(withJSONObject:object,options:.sortedKeys)
    }
    runtime.receive(packet(scene:"thinking",run:"1"))
    runtime.receive(packet(scene:"failure",run:nil))
    precondition((member(app,"currentScene") as? String) != "success","Failed run played success")
    runtime.receive(packet(scene:"thinking",run:"2"))
    runtime.receive(packet(scene:"declined",run:nil))
    precondition((member(app,"currentScene") as? String) != "success","Cancelled run played success")
    runtime.receive(try JSONSerialization.data(withJSONObject:["type":"quickVisibility","visible":true]))
    precondition(member(pets,"suppressed") as? Bool == true)
    runtime.receive(packet(scene:"thinking",run:"3"))
    precondition(member(pets,"suppressed") as? Bool == true,"Owner notification unsuppressed Quick Chat pets")
    precondition(member(app,"quickChatVisible") as? Bool == true)
    runtime.receive(packet(scene:"off",run:nil,finished:true))
    runtime.receive(try JSONSerialization.data(withJSONObject:["type":"quickVisibility","visible":false]))
    runtime.receive(packet(scene:"off",run:nil))
    runtime.receive(packet(scene:"thinking",run:"3"))
    runtime.receive(packet(scene:"off",run:nil,finished:true))
    // Confirmed result is deduped per epoch; a new observed run exercises another event.
    var fresh=try JSONSerialization.jsonObject(with:packet(scene:"off",run:nil,finished:true)) as! [String:Any]
    fresh["finishedTasks"]=[["id":"code:fixture","runId":"4"]]
    runtime.receive(try JSONSerialization.data(withJSONObject:fresh))
    precondition(member(app,"currentScene") as? String == "success","Auto-review replaced confirmed done indicator")
    let suite="KaiSidebarAudit-"+UUID().uuidString,localDefaults=UserDefaults(suiteName:suite)!
    defer{localDefaults.removePersistentDomain(forName:suite)}
    let ui=MiniAppInterface(defaults:localDefaults);ui.layout(in:CGRect(x:0,y:0,width:450,height:320))
    func viewState(run:String?,success:Bool)->KaiState{
        var object=try! JSONSerialization.jsonObject(with:packet(scene:run == nil ? "off":"thinking",run:run,finished:success)) as! [String:Any]
        object["standalone"]=false;object["projects"]=[["path":"/private/tmp","name":"Audit"]]
        object["chats"]=[["id":"code:fixture","name":"Fixture","agent":"codex","status":run == nil ? "idle":"active"]]
        return try! JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
    }
    func findRow(_ layer:CALayer)->CALayer?{
        if layer.name == "Fixture"{return layer}
        for child in layer.sublayers ?? []{if let found=findRow(child){return found}}
        return nil
    }
    ui.setState(viewState(run:"3",success:false))
    guard let row=findRow(ui.layer),let engine=member(row,"statusEngine") as? NotchIndicatorEngine else{fatalError("No sidebar row status engine")}
    ui.setState(viewState(run:nil,success:false))
    precondition(engine.mode == .off)
    ui.setState(viewState(run:nil,success:true))
    precondition(engine.mode != .off,"Late confirmed outcome lost its done badge")

    // Critical approval state must show the native, clickable controls even when
    // the workspace or floating Quick Chat owns ordinary presentation.
    app.revealWorkspace()
    let until=Date().addingTimeInterval(0.95);while Date()<until{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    var approval=try JSONSerialization.jsonObject(with:packet(scene:"permission",run:"5")) as! [String:Any]
    approval["approval"]=true;approval["approvalThreadId"]="code:fixture";approval["approvalRequestId"]="5:opaque";approval["approvalSummary"]="Run cargo test";approval["approvalCount"]=2
    approval["nativeReview"]=["owner":"code:fixture","token":"5:opaque","title":"Permission","detail":"cargo test","choices":[["id":"accept","label":"Allow once"],["id":"decline","label":"Decline"]],"questions":[]]
    runtime.receive(try JSONSerialization.data(withJSONObject:approval))
    guard let controls=member(app,"approvalUI") as? CompactApprovalView else{fatalError("Missing approval UI")}
    precondition(controls.window?.isVisible == true && controls.window?.ignoresMouseEvents == false,"Workspace hid approval controls")
    precondition(controls.request?.thread == "code:fixture" && controls.request?.id == "5:opaque")
    controls.onDecision?("approve",controls.request!)
    precondition(commands.last?["action"] as? String == "approve" && commands.last?["approvalRequestId"] as? String == "5:opaque","Native UI lost exact approval token")
    approval["approvalRequestId"]="6:opaque";var review=approval["nativeReview"] as! [String:Any];review["token"]="6:opaque";approval["nativeReview"]=review
    runtime.receive(try JSONSerialization.data(withJSONObject:["type":"quickVisibility","visible":true]))
    runtime.receive(try JSONSerialization.data(withJSONObject:approval))
    precondition(controls.window?.isVisible == true && controls.window?.ignoresMouseEvents == false,"Quick Chat hid approval controls")
    controls.onCycle?();precondition(commands.last?["action"] as? String == "approval_next")
    // A normal owner update omits the unchanged transcript. The native model
    // retains it for the same chat and clears it for a different owner.
    var history=try JSONSerialization.jsonObject(with:packet(scene:"off",run:nil)) as! [String:Any]
    history["history"]=[["id":"message","kind":"assistant","lines":[["text":"Retain this transcript","style":"text"]]]]
    runtime.receive(try JSONSerialization.data(withJSONObject:history))
    history.removeValue(forKey:"history");history["notice"]="Status changed"
    runtime.receive(try JSONSerialization.data(withJSONObject:history))
    precondition((member(app,"latestState") as? KaiState)?.history.first?.lines.first?.text == "Retain this transcript")
    history["threadId"]="code:other";runtime.receive(try JSONSerialization.data(withJSONObject:history))
    precondition((member(app,"latestState") as? KaiState)?.history.isEmpty == true,"Transcript leaked across owners")

    // Owner-created images become notch thumbnails without roundtripping through
    // the import command. An image-only draft reaches the normal native Send.
    let file=FileManager.default.temporaryDirectory.appendingPathComponent("kai-owner-image-\(UUID().uuidString).png")
    let bitmap=NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:8,pixelsHigh:8,bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0)!
    try bitmap.representation(using:.png,properties:[:])!.write(to:file);defer{try? FileManager.default.removeItem(at:file)}
    history["attachments"]=[file.path];history["composerDraft"]="";let beforeImports=commands.filter{$0["action"] as? String == "attachments"}.count
    runtime.receive(try JSONSerialization.data(withJSONObject:history))
    let imageDeadline=Date().addingTimeInterval(1);while Date()<imageDeadline && (member(app,"ownerAttachmentImages").map{Mirror(reflecting:$0).children.count} ?? 0)==0{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    precondition(member(app,"queuedImageCount") as? Int == 1,"Owner image count never reached notch")
    precondition(commands.filter{$0["action"] as? String == "attachments"}.count == beforeImports,"Owner thumbnail reimported its original")
    guard let mini=member(app,"miniUI") as? MiniAppInterface else{fatalError("Missing mini UI")}
    mini.onSend?();precondition(commands.last?["action"] as? String == "send" && commands.last?["text"] as? String == "","Image-only draft could not send from notch")
    print("Native state regression: failure/cancellation, late success, exclusive Quick Chat, visible clickable approvals over both surfaces, exact approval decisions, inbox cycling, incremental history, and image-only owner drafts passed")
 }
}
