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
    let runtime=EmbeddedCodeRuntime.shared
    var foreground=false;runtime.workspaceFocusProvider={foreground}
    runtime.start(window:window,command:auditCommand)
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

    // A busy text composer is not a read-only voice conversation. Actual
    // effective model/effort labels must render even with no model override.
    var voice=try JSONSerialization.jsonObject(with:packet(scene:"thinking",run:"voice")) as! [String:Any]
    voice["readOnly"]=false;voice["composerBusy"]=true;voice["workspaceVoiceActive"]=true
    voice["sessionActivity"]=["scene":"speaking","text":"Speaking · gpt-live-1-codex"]
    voice["effectiveModel"]="gpt-6-luna";voice["effectiveReasoningEffort"]="medium";voice["models"]=[["model":"gpt-6-luna","displayName":"GPT-6-Luna","supportedReasoningEfforts":[["reasoningEffort":"medium"]]]]
    let voiceState=try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:voice));ui.setState(voiceState)
    let status=(member(ui,"message") as? CATextLayer)?.string as? NSAttributedString
    precondition(status?.string.contains("Speaking") == true && status?.string.contains("read only") == false,"An active editable voice call retained its stale connecting/read-only header")
    let labels=(member(ui,"settingsBar") as? CALayer)?.sublayers?.compactMap{($0.mask as? CATextLayer)?.string as? String} ?? []
    precondition(labels.contains("gpt-6-luna") && labels.contains("medium"),"Resolved CLI model/effort were not rendered in the actual settings strip")

    // Ordinary compact -> mini -> workspace expansion keeps the owner’s selected
    // idle chat even while the activity indicator follows a different agent.
    var selected=try JSONSerialization.jsonObject(with:packet(scene:"thinking",run:nil)) as! [String:Any]
    selected["activeTasks"]=[["id":"code:other-running","name":"Other agent","runId":"other-run","scene":"thinking","text":"Working","agent":"codex"]]
    selected["focusThreadId"]="code:other-running";selected["composerDraft"]="Keep this draft"
    runtime.receive(try JSONSerialization.data(withJSONObject:selected))
    let priorNavigation=commands.filter{$0["action"] as? String == "chat"}.count
    app.expandToRunningChat()
    precondition((member(app,"latestState") as? KaiState)?.threadId == "code:fixture","Notch expansion switched away from the selected chat")
    precondition((member(app,"composerField") as? NSTextField)?.stringValue == "Keep this draft","Notch expansion replaced the current draft")
    precondition(commands.filter{$0["action"] as? String == "chat"}.count == priorNavigation,"Expanding implicitly navigated to another running agent")
    // A workspace behind another application stays open while its notch overlays
    // that application. Returning reuses the same window and native morph.
    foreground=true;app.revealWorkspace()
    let until=Date().addingTimeInterval(0.95);while Date()<until{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    let retainedFrame=window.frame,retainedWindow=window.windowNumber
    let statusPanel=member(app,"panel") as! NSPanel
    precondition(!statusPanel.isVisible,"Foreground Code duplicated its status notch")
    let focusCommands=commands.count
    foreground=false
    NSWorkspace.shared.notificationCenter.post(name:NSWorkspace.didActivateApplicationNotification,object:nil)
    RunLoop.current.run(until:Date().addingTimeInterval(0.03))
    precondition(statusPanel.isVisible && member(app,"open") as? Bool == true && member(app,"miniOpen") as? Bool == false,"Switching to another app did not automatically show the compact status notch")
    precondition(statusPanel.ignoresMouseEvents && !statusPanel.isKeyWindow,"Background status stole keyboard focus")
    precondition(window.isVisible && window.frame==retainedFrame && window.windowNumber==retainedWindow,"Showing background status hid or replaced Code")
    precondition(commands.count==focusCommands,"Switching apps changed the owner or replayed the workspace morph")
    precondition((member(app,"composerField") as? NSTextField)?.stringValue == "Keep this draft","Background status lost the selected chat draft")
    foreground=true;app.workspaceFocusChanged()
    precondition(!statusPanel.isVisible && window.isVisible,"Returning to Code left a duplicate notch or closed Code")
    foreground=false;NotificationCenter.default.post(name:NSApplication.didResignActiveNotification,object:NSApp)
    RunLoop.current.run(until:Date().addingTimeInterval(0.03))
    precondition(statusPanel.isVisible,"A second app switch did not restore background status")
    // Losing focus during the reveal must be reconciled when its contour
    // finishes, even if no further activation notification follows.
    foreground=true;app.revealWorkspace()
    foreground=false;app.workspaceFocusChanged()
    let lostDuringReveal=Date().addingTimeInterval(0.95);while Date()<lostDuringReveal{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    precondition(statusPanel.isVisible && window.isVisible,"Focus loss during workspace reveal never restored its background notch")
    var idle=selected;idle["activeTasks"]=[];idle["activeTurnId"]=NSNull();idle["activity"]=["scene":"off","text":"Ready"];idle["notice"]="Background idle"
    runtime.receive(try JSONSerialization.data(withJSONObject:idle))
    let idleDeadline=Date().addingTimeInterval(1.3);while Date()<idleDeadline{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    precondition(statusPanel.isVisible && member(app,"miniOpen") as? Bool == false,"An idle update hid the persistent background status notch")
    runtime.receive(try JSONSerialization.data(withJSONObject:selected))
    app.collapseWorkspace()
    precondition(window.isVisible && window.frame==retainedFrame && window.windowNumber==retainedWindow,"Overlay hid or replaced Code")
    precondition(member(app,"workspaceNotchOverlay") as? Bool == true && member(app,"miniOpen") as? Bool == true,"Background shortcut did not open extended notch")
    app.dismissUnifiedSurface();precondition(window.isVisible,"Dismissing the overlay closed Code")
    app.collapseWorkspace();app.expandWorkspace()
    precondition(commands.last?["phase"] as? String == "reopen","Returning to Code skipped the native morph")
    let reopened=Date().addingTimeInterval(0.95);while Date()<reopened{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    precondition(window.isVisible && window.windowNumber==retainedWindow && window.frame==retainedFrame,"Morph return replaced the workspace")
    foreground=true
    // Direct Code -> extended notch: one contour target, with Code pixels
    // retired before the real mini UI animates concurrently above the glass.
    let workspaceFrame=window.frame
    let workspaceSprite=member(app,"sprite") as! CALayer,workspaceSpriteFrame=workspaceSprite.frame
    let nativePanel=member(app,"panel") as! NSPanel,nativeLevel=nativePanel.level
    let miniHeight=member(app,"canvasHeight") as! CGFloat
    let compactHeight=member(app,"notchHeight") as! CGFloat
    app.collapseWorkspace()
    guard let handoff=member(app,"workspaceMorph") as? WorkspaceMorph,
          let handoffClip=member(handoff,"clip") as? CAShapeLayer,
          let codeHost=member(handoff,"host") as? NSView else{fatalError("Missing direct notch handoff")}
    precondition(window.frame==workspaceFrame && workspaceSprite.frame==workspaceSpriteFrame,"Handoff resized Code or moved its shared pet before retirement")
    precondition(abs((handoffClip.path?.boundingBoxOfPath.height ?? 0)-miniHeight)<0.1,"Workspace took an extra compact-notch step")
    precondition(codeHost.layer?.opacity==0 && member(app,"miniOpen") as? Bool == false,"Code content did not begin retirement before mini appeared")
    let overlap=Date().addingTimeInterval(0.23);while Date()<overlap{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    guard let notchMask=member(app,"mask") as? CAShapeLayer,let nativeMorph=notchMask.animation(forKey:"width") as? CABasicAnimation else{fatalError("The real extended notch did not use its original morph during the workspace collapse")}
    precondition(handoff.transitioning && window.isVisible && nativePanel.isVisible && member(app,"miniOpen") as? Bool == true,"The native notch did not overlap the direct workspace morph")
    precondition(nativePanel.level.rawValue>window.level.rawValue && codeHost.layer?.opacity==0,"Code's UI could cover the extended notch")
    precondition((codeHost.layer?.presentation()?.opacity ?? 0)<0.01,"Workspace pixels remained visible inside the destination notch")
    let nativeStart=nativeMorph.fromValue as! CGPath,nativeEnd=nativeMorph.toValue as! CGPath
    precondition(abs(nativeStart.boundingBoxOfPath.height-compactHeight)<0.1 && abs(nativeEnd.boundingBoxOfPath.height-miniHeight)<0.1,"Handoff replaced Kai's original native notch morph")
    let foldedMini=Date().addingTimeInterval(0.5);while Date()<foldedMini{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    precondition(!window.isVisible && nativePanel.isVisible && nativePanel.level==nativeLevel,"Direct handoff lost its surface or original panel level")
    precondition(notchMask.animation(forKey:"width")==nil,"A second notch expansion ran after Code had collapsed")
    precondition((member(app,"composerField") as? NSTextField)?.stringValue == "Keep this draft","Native notch handoff lost the selected chat draft")
    // Reverse after the native mini has begun. No delayed completion may reopen
    // it over Code, and the workspace content must regain its original opacity.
    app.revealWorkspace();let reopenedCode=Date().addingTimeInterval(1);while Date()<reopenedCode{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    app.collapseWorkspace();let reverseAt=Date().addingTimeInterval(0.23);while Date()<reverseAt{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    app.revealWorkspace();let expandedAgain=Date().addingTimeInterval(1);while Date()<expandedAgain{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    precondition(window.isVisible && !nativePanel.isVisible && codeHost.layer?.opacity==1 && nativePanel.level==nativeLevel,"Reversing the live handoff left a stale notch or invisible workspace")
    let originalLevel=window.level
    window.setFrame(CGRect(x:retainedFrame.minX+90,y:retainedFrame.minY+80,width:850,height:580),display:true)
    app.dismissUnifiedSurface()
    guard let morph=member(app,"workspaceMorph") as? WorkspaceMorph,let shell=member(morph,"shell") as? NSPanel,let notch=member(app,"panel") as? NSPanel else{fatalError("Windowed collapse did not create a continuous native contour")}
    precondition(shell.level.rawValue>NSWindow.Level.statusBar.rawValue,"Windowed collapse was hidden underneath the menu bar")
    precondition(abs(shell.frame.maxY-notch.frame.maxY)<0.1,"Windowed collapse ended at a synthetic notch below the real screen edge")
    let folded=Date().addingTimeInterval(0.8);while Date()<folded{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    precondition(!window.isVisible && window.level==originalLevel,"Windowed collapse left a floating window or lost its original level")
    app.revealWorkspace();let restored=Date().addingTimeInterval(1);while Date()<restored{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    var approval=try JSONSerialization.jsonObject(with:packet(scene:"permission",run:"5")) as! [String:Any]
    approval["composerDraft"]="Keep this draft"
    approval["approval"]=true;approval["approvalThreadId"]="code:fixture";approval["approvalRequestId"]="5:opaque";approval["approvalSummary"]="Run cargo test";approval["approvalCount"]=2
    approval["nativeReview"]=["owner":"code:fixture","token":"5:opaque","title":"Permission","detail":"cargo test","choices":[["id":"accept","label":"Allow once"],["id":"decline","label":"Decline"]],"questions":[]]
    runtime.receive(try JSONSerialization.data(withJSONObject:approval))
    guard let controls=member(app,"approvalUI") as? CompactApprovalView else{fatalError("Missing approval UI")}
    precondition(member(app,"approvalPopout") as? Bool == false,"Foreground Code approval stole the native review surface")
    foreground=false;NotificationCenter.default.post(name:NSApplication.didResignActiveNotification,object:NSApp)
    RunLoop.current.run(until:Date().addingTimeInterval(0.03))
    precondition(controls.window?.isVisible == true && controls.window?.ignoresMouseEvents == false,"Background approval never reached Kai")
    foreground=true;app.workspaceFocusChanged()
    precondition(member(app,"approvalPopout") as? Bool == false,"Returning to Code left a duplicate notch approval")
    foreground=false;app.workspaceFocusChanged()
    precondition(controls.window?.isVisible == true,"Same pending approval did not follow Code back into the background")
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
    runtime.receive(try JSONSerialization.data(withJSONObject:["type":"quickVisibility","visible":false]))
    var history=try JSONSerialization.jsonObject(with:packet(scene:"off",run:nil)) as! [String:Any]
    history["history"]=[["id":"message","kind":"assistant","lines":[["text":"Retain this transcript","style":"text"]]]]
    history["workspaceTextMorph"]=true
    runtime.receive(try JSONSerialization.data(withJSONObject:history))
    app.expandToRunningChat()
    let textDecode=member(app,"textReveal") as! ConversationTextReveal
    let nativeText=member(app,"conversationText") as! NSTextView
    precondition(textDecode.activeTileCount>0 && textDecode.snapshotCount>0,"Unified extended notch skipped native text unjumbling")
    let textSettled=Date().addingTimeInterval(0.45);while Date()<textSettled{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    precondition(textDecode.activeTileCount==0 && nativeText.string.contains("Retain this transcript"),"Unjumbling changed or failed to restore the real transcript")
    app.dismissUnifiedSurface();let closedText=Date().addingTimeInterval(0.3);while Date()<closedText{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    app.expandToRunningChat()
    precondition(textDecode.activeTileCount>0,"Reopening the unchanged selected chat did not replay visible text unjumbling")
    app.revealWorkspace();let codeText=Date().addingTimeInterval(1);while Date()<codeText{RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
    foreground=true;app.collapseWorkspace();let decodeOverlap=Date().addingTimeInterval(0.23);while Date()<decodeOverlap{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    precondition(textDecode.activeTileCount>0,"Direct Code-to-notch handoff lost the native text unjumbling")
    let decodedHandoff=Date().addingTimeInterval(0.5);while Date()<decodedHandoff{RunLoop.current.run(until:Date().addingTimeInterval(0.005))}
    precondition(textDecode.activeTileCount==0 && nativeText.string.contains("Retain this transcript"),"Handoff text failed to settle into the real native glyphs")
    foreground=false
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
    print("Native state regression: failure/cancellation, late success, exclusive Quick Chat, automatic nonactivating background status, retained background workspace, replayed morph, focus-aware clickable approvals, selected-chat/draft continuity, exact approval decisions, inbox cycling, incremental history, and image-only owner drafts passed")
 }
}
