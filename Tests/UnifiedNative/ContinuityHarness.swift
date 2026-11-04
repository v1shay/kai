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
        var owner="code:continuous",thread="code:continuous",draft="",results=[[String:Any]](),attachments=[String](),inputBlocked=false
        func publish(){
            let value:[String:Any]=["ownerToken":owner,"composerDraft":draft,"project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":thread,"threadName":"Continuous chat","history":[],"readOnly":false,"composerBusy":inputBlocked,"approval":false,"activity":["scene":"thinking","text":"Working"],"sessionActivity":["scene":"thinking","text":"Working"],"activeTurnId":"same-running-task","activeTasks":[],"notice":"Ready","attachments":attachments,"completedRequestError":"Unrelated command failed","selectedAgent":"codex","models":[],"workspaceMotion":false,"completedRequestId":"unrelated-later-command","completedRequests":results]
            runtime.receive(try! JSONSerialization.data(withJSONObject:value))
        }
        func edit(_ text:String){field.stringValue=text;app.controlTextDidChange(Notification(name:NSControl.textDidChangeNotification,object:field))}
        func last(_ action:String)->[String:Any]{commands.last{$0["action"] as? String == action}!}
        func acknowledge(_ request:[String:Any],_ text:String,error:String?=nil){
            var reply:[String:Any]=["id":request["requestId"]!];if let error{reply["error"]=error};results.append(reply);draft=text;publish()
        }
        publish();app.expandToRunningChat()
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
        print("Native continuity: rapid draft writes, repeated surface handoffs, task identity, ledger acknowledgements, repeated failed sends, next-draft preservation, and Ninja owner routing passed")
    }
}
