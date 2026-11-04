import AppKit

private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool {
    guard let bytes,let fields=try? JSONSerialization.jsonObject(with:Data(bytes:bytes,count:count)) as? [String:Any] else{return false}
    MainActor.assumeIsolated {
        MorphMatrixAudit.commands.append(fields)
        if fields["action"] as? String == "conversation_surface" {
            var reply=fields;reply["type"]="conversationSurface"
            EmbeddedCodeRuntime.shared.receive(try! JSONSerialization.data(withJSONObject:reply))
        }
    }
    return true
}
@MainActor private final class FocusAuditWindow:NSWindow {
    override func makeKeyAndOrderFront(_ sender:Any?){precondition(NSApp.isActive,"Key focus requested before application activation");super.makeKeyAndOrderFront(sender)}
}
private func member(_ object:Any,_ key:String)->Any?{guard let value=Mirror(reflecting:object).children.first(where:{$0.label == key})?.value else{return nil};let mirror=Mirror(reflecting:value);return mirror.displayStyle == .optional ? mirror.children.first?.value:value}

@main struct MorphMatrixAudit {
    @MainActor static var commands=[[String:Any]]()
    @MainActor static func main()throws {
        _=NSApplication.shared
        UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded")
        for key in ["soundsEnabled","speakResponses","spotifyMedia","youtubeMedia"]{UserDefaults.standard.set(false,forKey:key)}
        let workspace=FocusAuditWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
        workspace.isReleasedWhenClosed=false
        let originalContent=workspace.contentView!,originalNumber=workspace.windowNumber
        let runtime=EmbeddedCodeRuntime.shared;var foreground=true
        runtime.workspaceFocusProvider={foreground};runtime.start(window:workspace,command:command)
        let app=member(runtime,"app") as! KaiPetApp,panel=member(app,"panel") as! NSPanel,morph=member(app,"workspaceMorph") as! WorkspaceMorph
        defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
        let state:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:matrix","threadName":"Matrix","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"sessionActivity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Ready","attachments":[],"selectedAgent":"codex","models":[],"ownerToken":"matrix","composerDraft":"Shared draft"]
        func publish(_ value:[String:Any]){runtime.receive(try! JSONSerialization.data(withJSONObject:value))}
        func pump(_ seconds:Double){RunLoop.current.run(until:Date().addingTimeInterval(seconds))}
        func settle(){pump(0.02);let end=Date().addingTimeInterval(3);while morph.transitioning && Date()<end{pump(0.02)};precondition(!morph.transitioning,"Morph never completed")}
        func verify(_ form:ConversationForm){
            precondition(workspace.isVisible && !workspace.ignoresMouseEvents && workspace.contentView?.layer?.opacity==1,"Conversation did not regain input and visibility")
            precondition(runtime.workspaceWindow === workspace && workspace.windowNumber==originalNumber && workspace.contentView === originalContent,"Form replaced the original window or content")
            precondition(member(app,"conversationForm") as? ConversationForm == form)
            precondition(!panel.isVisible && member(app,"miniOpen") as? Bool == false && member(app,"quickWindow")==nil,"A competing interactive surface opened")
            precondition(workspace.level == (form == .workspace ? .normal:.floating),"Wrong settled window level")
            precondition(app.unifiedVoiceHost() === originalContent,"Voice moved to a hidden editor")
        }
        publish(state)
        for from in [ConversationForm.workspace,.notch,.mini,.menu] {
            for to in [ConversationForm.workspace,.notch,.mini,.menu] {
                app.requestConversation(from);settle();verify(from)
                app.requestConversation(to);settle();verify(to)
            }
        }
        // Reversal must retire the previous completion, not the current editor.
        for _ in 0..<8{app.requestConversation(.workspace);pump(0.025);app.requestConversation(.notch);pump(0.025);app.requestConversation(.mini);pump(0.025)}
        app.requestConversation(.menu);settle();pump(0.7);verify(.menu)
        morph.motionEnabled=false
        for form in [ConversationForm.notch,.workspace,.mini,.menu]{app.requestConversation(form);settle();verify(form)}
        morph.motionEnabled=true
        app.requestConversation(.mini);settle()
        var approval=state;approval["approval"]=true;approval["approvalThreadId"]="code:matrix";approval["approvalRequestId"]="matrix:approval";approval["approvalSummary"]="Fixture approval";approval["approvalCount"]=1
        approval["nativeReview"]=["owner":"code:matrix","token":"matrix:approval","title":"Permission","detail":"Run fixture","choices":[["id":"accept","label":"Allow once"],["id":"decline","label":"Decline"]],"questions":[]]
        publish(approval);pump(0.05)
        precondition(member(app,"approvalPopout") as? Bool == false,"Foreground conversation duplicated approval in the notch")
        foreground=false;app.workspaceFocusChanged();pump(0.1)
        precondition(member(app,"approvalPopout") as? Bool == true,"Background approval did not reach the notch")
        foreground=true;app.workspaceFocusChanged();pump(0.1)
        precondition(member(app,"approvalPopout") as? Bool == false,"Returning left a duplicate approval")
        for form in [ConversationForm.notch,.menu,.workspace]{app.requestConversation(form);settle();verify(form);precondition((member(app,"latestState") as? KaiState)?.approvalRequestId=="matrix:approval")}
        publish(state)
        app.requestConversation(.menu);pump(0.025);foreground=false;app.toggleMenuBarPanel();pump(0.9)
        precondition(!workspace.isVisible,"Repeated status click during opening failed to dismiss after losing key focus")
        foreground=true
        app.requestConversation(.notch);pump(0.025);app.dismissUnifiedSurface();pump(0.8)
        precondition(!workspace.isVisible,"A stale morph reopened the dismissed editor")
        app.requestConversation(.menu);settle();foreground=false;app.workspaceFocusChanged();pump(0.1)
        precondition(!workspace.isVisible,"Menu did not dismiss on focus loss")
        precondition((member(app,"latestState") as? KaiState)?.threadId=="code:matrix")
        precondition(!commands.contains{["draft","send","new","interrupt","chat","approve","decline","voice_stop"].contains($0["action"] as? String ?? "")},"Presentation mutated the conversation")
        print("Native morph matrix passed: all 16 form pairs, rapid reversals, reduced motion, one window/content/voice host, approval foreground routing, focus-loss dismissal, no conversation commands")
    }
}
