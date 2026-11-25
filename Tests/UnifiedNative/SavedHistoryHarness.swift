import AppKit

private func member(_ value:Any,_ name:String)->Any?{
    guard let item=Mirror(reflecting:value).children.first(where:{$0.label==name})?.value else{return nil}
    let mirror=Mirror(reflecting:item)
    return mirror.displayStyle == .optional ? mirror.children.first?.value:item
}
private func audit(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{
    guard let bytes,let object=try? JSONSerialization.jsonObject(with:Data(bytes:bytes,count:count)) as? [String:Any] else{return false}
    MainActor.assumeIsolated{SavedHistoryHarness.commands.append(object)};return true
}
@main struct SavedHistoryHarness {
    @MainActor static var commands=[[String:Any]]()
    @MainActor static func main()throws{
        _=NSApplication.shared
        let path=ProcessInfo.processInfo.environment["KAI_SAVED_HISTORY_PACKET"]!
        let packet=try Data(contentsOf:URL(fileURLWithPath:path))
        let state=try JSONDecoder().decode(KaiState.self,from:packet)
        let window=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
        window.isReleasedWhenClosed=false
        let runtime=EmbeddedCodeRuntime.shared;runtime.workspaceFocusProvider={false};runtime.start(window:window,command:audit)
        guard let app=member(runtime,"app") as? KaiPetApp else{fatalError("Native app missing")}
        defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
        runtime.receive(packet);app.expandToRunningChat()
        let text=member(app,"conversationText") as! NSTextView
        precondition(!text.string.contains("\\u003c") && !text.string.contains("\"orbit_interaction\"") && !text.string.contains("diff --git"),"Saved tool source leaked into the actual extended notch")
        precondition(text.string.count<25000,"Old tool details still flooded the native notch")
        var link:URL?
        text.textStorage?.enumerateAttribute(.link,in:NSRange(location:0,length:text.textStorage?.length ?? 0)){value,_,stop in
            if let url=value as? URL,url.scheme=="kai-interaction"{link=url;stop.pointee=true}
        }
        let selected=link!,id=URLComponents(url:selected,resolvingAgainstBaseURL:false)!.queryItems!.first!.value!
        precondition(state.history.contains{$0.detailId==id},"Native disclosure lost exact saved-card identity")
        precondition(app.textView(text,clickedOnLink:selected,at:0),"Native status title did not handle disclosure")
        precondition(commands.contains{($0["action"] as? String)=="interaction_details" && ($0["id"] as? String)==id},"Native status did not request the exact Code card")
        precondition((member(app,"workspaceMorph") as? WorkspaceMorph)?.expanded==true,"Saved-card disclosure skipped the native workspace morph")
        print("Actual saved reply native regression: \(state.history.count) compact rows, no escaped tool spill, exact detail dispatch and native workspace morph passed")
    }
}
