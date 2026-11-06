import AppKit

private func member(_ value:Any,_ name:String)->Any?{
    guard let child=Mirror(reflecting:value).children.first(where:{$0.label==name}) else{return nil}
    let mirror=Mirror(reflecting:child.value)
    return mirror.displayStyle == .optional ? mirror.children.first?.value:child.value
}
private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{
    guard let bytes else{return false}
    MainActor.assumeIsolated{
        if let value=try? JSONSerialization.jsonObject(with:Data(bytes:bytes,count:count)) as? [String:Any]{ContinuityAudit.commands.append(value)}
    }
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
        let runtime=EmbeddedCodeRuntime.shared;runtime.workspaceFocusProvider={false};runtime.start(window:window,command:command)
        let app=member(runtime,"app") as! KaiPetApp
        let ui=member(app,"miniUI") as! MiniAppInterface
        let field=member(app,"composerField") as! NSTextField
        defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
        var owner="code:continuous",thread="code:continuous",draft="",results=[[String:Any]](),attachments=[String](),inputBlocked=false,running=true
        func publish(){
            let value:[String:Any]=["ninjaGroups":[["id":"ninja-profile:host:empty","name":"Empty Ninja","style":"ice","chats":[]]],"ownerToken":owner,"composerDraft":draft,"project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":thread,"threadName":"Continuous chat","history":[],"readOnly":false,"composerBusy":inputBlocked,"approval":false,"activity":["scene":running ? "thinking":"off","text":running ? "Working":"Ready"],"sessionActivity":["scene":running ? "thinking":"off","text":running ? "Working":"Ready"],"activeTurnId":running ? "same-running-task" as Any:NSNull(),"activeTasks":running && owner.hasPrefix("personal:") ? [["id":"ninja:host:run","name":"Ninja","scene":"thinking","text":"Working","agent":"ninja","ninjaStyle":"ice"]]:[],"notice":"Ready","attachments":attachments,"completedRequestError":"Unrelated command failed","selectedAgent":"codex","models":[],"workspaceMotion":false,"completedRequestId":"unrelated-later-command","completedRequests":results]
            runtime.receive(try! JSONSerialization.data(withJSONObject:value))
        }
        func edit(_ text:String){field.stringValue=text;app.controlTextDidChange(Notification(name:NSControl.textDidChangeNotification,object:field))}
        func last(_ action:String)->[String:Any]{commands.last{$0["action"] as? String == action}!}
        func acknowledge(_ request:[String:Any],_ text:String,error:String?=nil){
            var reply:[String:Any]=["id":request["requestId"]!];if let error{reply["error"]=error};results.append(reply);draft=text;publish()
        }
        publish();app.expandToRunningChat()
        let rows=Mirror(reflecting:member(ui,"ninjaRows")!).children.map { element -> (CALayer,String) in
            let pair=Array(Mirror(reflecting:element.value).children)
            return (pair[0].value as! CALayer,pair[1].value as! String)
        }
        precondition(rows.count==1 && rows[0].1=="ninja-profile:host:empty","A Ninja without task history is missing from the native sidebar")
        let left=member(ui,"left") as! CALayer
        let target=rows[0].0.frame.offsetBy(dx:left.frame.minX,dy:left.frame.minY)
        precondition(ui.handleClick(CGPoint(x:target.midX,y:target.midY)))
        precondition(last("chat")["id"] as? String == "ninja-profile:host:empty","Native Ninja row did not navigate its owner")
        acknowledge(last("chat"),draft)
        let navigation=ui.beginChatLoading("Another chat")
        results.append(["id":navigation]);publish()
        precondition(ui.pendingRequestID==nil,"Navigation stayed loading when its acknowledgement was followed by another command")
        edit("h");let first=last("draft")
        edit("hello from the notch");let second=last("draft")
        precondition(first["draftBase"] as? String == "")
        precondition(second["draftBase"] as? String == "h","Rapid typing reused a stale frame's draft base")
        acknowledge(first,"h")
        precondition(field.stringValue == "hello from the notch","Intermediate acknowledgement rolled back typing")
        acknowledge(second,"hello from the notch")
        for _ in 0..<3{
            app.expandWorkspace();app.foldWorkspace();app.expandToRunningChat();app.prepareQuickOpening();app.setQuickChatVisible(false)
            precondition(field.stringValue == draft,"Presentation switching discarded the shared draft")
            precondition(!commands.contains{$0["action"] as? String == "interrupt" || $0["action"] as? String == "new"},"A presentation transition mutated task ownership")
        }
        ui.onSend?();let sent=last("send")
        precondition(sent["targetThreadId"] as? String == thread)
        precondition(sent["draftBase"] as? String == draft)
        acknowledge(sent,"")
        precondition(field.stringValue.isEmpty,"A reply hidden behind a newer command failed to clear the accepted draft")
        precondition(member(app,"sendingPrompt")==nil,"Send stayed locked after its acknowledgement")
        for _ in 0..<2{
            edit("retry me");acknowledge(last("draft"),"retry me")
            let before=commands.filter{$0["action"] as? String == "send"}.count
            ui.onSend?();let retry=last("send")
            precondition(commands.filter{$0["action"] as? String == "send"}.count==before+1,"A repeated failure locked Send")
            acknowledge(retry,"retry me",error:"Host unavailable")
            precondition(field.stringValue == "retry me","Failed send discarded the draft")
            precondition(member(app,"sendingPrompt")==nil,"Identical errors left Send permanently locked")
        }
        ui.onSend?();let delayed=last("send")
        edit("next message typed while sending")
        acknowledge(delayed,"")
        precondition(field.stringValue == "next message typed while sending","Completion erased the next unsent message")
        precondition(last("draft")["text"] as? String == field.stringValue)
        acknowledge(last("draft"),field.stringValue)
        ui.onSend?();let disconnected=last("send")
        runtime.onError?("Transport interrupted")
        edit("preserve this after disconnect")
        acknowledge(disconnected,"")
        precondition(field.stringValue == "preserve this after disconnect","A late reply after transport failure erased newer input")
        precondition(member(app,"sendingPrompt")==nil && member(app,"sendingPromptRequestID")==nil)
        let picture=NSImage(size:NSSize(width:8,height:8));picture.lockFocus();NSColor.red.setFill();NSRect(x:0,y:0,width:8,height:8).fill();picture.unlockFocus()
        ui.onImagesChanged?([picture]);ui.onSend?()
        let deadline=Date().addingTimeInterval(3)
        while Date()<deadline && !commands.contains(where:{$0["action"] as? String == "attachments"}){RunLoop.current.run(until:Date().addingTimeInterval(0.01))}
        let imported=last("attachments")
        defer{for path in imported["paths"] as? [String] ?? []{try? FileManager.default.removeItem(atPath:path)}}
        let sendsBeforeImport=commands.filter{$0["action"] as? String == "send"}.count
        inputBlocked=true;acknowledge(imported,draft)
        precondition(commands.filter{$0["action"] as? String == "send"}.count==sendsBeforeImport,"Image send ran before host import completed")
        attachments=["/private/tmp/imported-owner-image.png"];inputBlocked=false;publish()
        RunLoop.current.run(until:Date().addingTimeInterval(0.1))
        precondition(commands.filter{$0["action"] as? String == "send"}.count==sendsBeforeImport+1,"An unrelated command error blocked image submission")
        acknowledge(last("send"),"");attachments=[]
        let importsBeforeNavigation=commands.filter{$0["action"] as? String == "attachments"}.count
        ui.onImagesChanged?([picture]);ui.onSend?()
        owner="personal:host:ninja:chat";thread="ninja-chat:host:ninja:chat";draft="Ninja's own draft";publish()
        RunLoop.current.run(until:Date().addingTimeInterval(0.1))
        precondition(commands.filter{$0["action"] as? String == "attachments"}.count==importsBeforeNavigation,"Pending image import followed navigation into a different owner")
        precondition(field.stringValue == draft,"Explicit chat navigation did not adopt the selected owner's draft")
        edit("Ninja continuation");let ninjaDraft=last("draft")
        precondition(ninjaDraft["ownerToken"] as? String == owner && ninjaDraft["targetThreadId"] as? String == thread)
        acknowledge(ninjaDraft,"Ninja continuation")
        ui.onSend?();let ninjaSend=last("send")
        precondition(ninjaSend["ownerToken"] as? String == owner && ninjaSend["text"] as? String == draft)
        acknowledge(ninjaSend,"")
        precondition(field.stringValue.isEmpty && member(app,"sendingPrompt")==nil)
        app.foldWorkspace();app.prepareQuickOpening();app.setQuickChatVisible(false)
        publish();running=false;publish()
        RunLoop.current.run(until:Date().addingTimeInterval(1.5))
        let settled=member(app,"latestState") as! KaiState
        precondition(settled.activeTasks.isEmpty && settled.activeTurnId==nil)
        precondition(member(app,"compactWhileWorking") as? Bool == false,"Compact notch retained work after the authoritative task disappeared")
        let sidebarState:[String:Any]=["project":"/private/tmp","projects":(0..<80).map{["name":"Project \($0)","path":"/tmp/project-\($0)"]},"chats":[],"standaloneChats":[],"standalone":true,"threadName":"Scroll test","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Ready","attachments":[]]
        ui.setState(try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:sidebarState)))
        ui.layout(in:CGRect(x:0,y:0,width:460,height:250))
        let rail=member(ui,"sidebarRail") as! CALayer,thumb=member(ui,"sidebarThumb") as! CALayer
        precondition(!rail.isHidden && thumb.frame.height<rail.frame.height,"Overflow has no visible scrollbar")
        let start=CGPoint(x:left.frame.minX+rail.frame.midX,y:left.frame.minY+rail.frame.minY+thumb.frame.midY)
        precondition(ui.beginSidebarScroll(at:start))
        ui.dragSidebarScroll(to:CGPoint(x:start.x,y:start.y-80));ui.endSidebarScroll()
        precondition((member(ui,"projectOffset") as! CGFloat)>0,"Dragging the sidebar thumb did not scroll")
        precondition(ui.renderedSidebarRowCount<30,"Sidebar stopped virtualizing offscreen rows")
        print("Native continuity: rapid draft writes, repeated surface handoffs, task identity, ledger acknowledgements, repeated failed sends, next-draft preservation, and Ninja owner routing passed")
    }
}
