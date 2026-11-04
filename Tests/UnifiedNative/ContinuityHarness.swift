import AppKit

private func member(_ value:Any,_ name:String)->Any?{
    guard let child=Mirror(reflecting:value).children.first(where:{$0.label==name}) else{return nil}
    let mirror=Mirror(reflecting:child.value)
    return mirror.displayStyle == .optional ? mirror.children.first?.value:child.value
}
private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{
    guard let bytes else{return false}
    MainActor.assumeIsolated{if let value=try? JSONSerialization.jsonObject(with:Data(bytes:bytes,count:count)) as? [String:Any]{ContinuityAudit.commands.append(value)}}
    return true
}
@main struct ContinuityAudit{
    @MainActor static var commands=[[String:Any]]()
    @MainActor static func main()throws{
        _=NSApplication.shared
        for key in ["soundsEnabled","spotifyMedia","youtubeMedia"]{UserDefaults.standard.set(false,forKey:key)}
        UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded")
        let window=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
        window.isReleasedWhenClosed=false
        let content=window.contentView!,windowNumber=window.windowNumber
        let runtime=EmbeddedCodeRuntime.shared;runtime.workspaceFocusProvider={true};runtime.start(window:window,command:command)
        let app=member(runtime,"app") as! KaiPetApp
        defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
        let state:[String:Any]=["ownerToken":"personal:host:ninja:chat","composerDraft":"unchanged draft","project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"ninja-chat:host:ninja:chat","threadName":"Shared chat","history":[],"readOnly":false,"approval":false,"activity":["scene":"thinking","text":"Working"],"activeTurnId":"same-task","activeTasks":[],"notice":"Ready","attachments":[],"selectedAgent":"codex","models":[],"workspaceMotion":false]
        runtime.receive(try JSONSerialization.data(withJSONObject:state))
        for _ in 0..<3 {
            for form in [ConversationForm.workspace,.notch,.mini,.menu] {
                app.requestConversation(form)
                let request=commands.last!
                precondition(request["action"] as? String == "conversation_surface")
                app.presentConversation(form,revision:request["revision"] as! Int)
                RunLoop.current.run(until:Date().addingTimeInterval(0.15))
                precondition(runtime.workspaceWindow === window && window.windowNumber==windowNumber)
                precondition(window.contentView === content,"Presentation replaced the live conversation view")
                precondition(member(app,"miniOpen") as? Bool == false,"The duplicate native editor was opened")
                precondition(member(app,"quickWindow")==nil,"A second conversation window was created")
                precondition(window.frame.width>=480 && window.frame.height>=360)
                precondition((member(app,"latestState") as! KaiState).activeTurnId=="same-task")
            }
        }
        // A queued show cannot resurrect a window after a later dismissal.
        app.requestConversation(.notch);let stale=commands.last!
        app.dismissUnifiedSurface()
        app.presentConversation(.notch,revision:stale["revision"] as! Int)
        precondition(!window.isVisible)
        precondition(!commands.contains{["draft","send","new","interrupt","chat"].contains($0["action"] as? String ?? "")},"Resizing mutated conversation state")
        for form in [ConversationForm.workspace,.notch,.mini,.menu]{
            for screen in [CGRect(x:0,y:0,width:1024,height:700),CGRect(x:-1920,y:0,width:1920,height:1080)]{
                let frame=form.frame(in:screen,workspace:CGRect(x:9999,y:9999,width:1400,height:1000),anchor:CGRect(x:9999,y:9999,width:20,height:20))
                precondition(screen.contains(frame),"Surface escaped the current display")
            }
        }
        print("Native continuity: same NSWindow and content view across all forms; no draft transport; stale presentation rejected; display geometry bounded")
    }
}
